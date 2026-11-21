//! Signing this install in to an account: `rvrs login`, `rvrs logout`, and the credential every
//! later request carries.
//!
//! The flow is the device authorization grant (RFC 8628) as a person meets it: the command shows
//! a short code and a link, the person confirms the code in a browser where they are signed in,
//! and the command, which has been asking every few seconds, receives its credential. rvrs talks
//! to the rvrs API for this and never to the identity provider, so no client secret ships in an
//! open source binary, and the credential is one the API issues and can revoke.
//!
//! A device nobody sits at takes a token instead: `RVRS_TOKEN`, made in the web app.

use std::io::{IsTerminal, Write};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use rvrs_core::paths;
use serde::{Deserialize, Serialize};

use crate::help::VERSION;
use crate::style::{ColorChoice, Style};
use crate::term;

const DEFAULT_API: &str = "https://api.rvrs.dev";

/// Where the API is. `RVRS_API` points at another one: a self-hosted rvrs, or a local one.
pub fn api_base() -> String {
    std::env::var("RVRS_API")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_API.to_string())
}

/// What `rvrs login` keeps: the credential and enough about the account to say who is signed in
/// without asking the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credential {
    /// The API the token belongs to. A token is never sent to another one.
    pub api: String,
    pub token: String,
    pub email: String,
    pub workspace: String,
    /// What this install is called in the account.
    pub install: String,
}

fn credential_path() -> std::path::PathBuf {
    paths::config_dir().join("credentials.json")
}

impl Credential {
    /// The stored credential, when there is one for the API in use.
    pub fn stored() -> Option<Self> {
        let text = std::fs::read_to_string(credential_path()).ok()?;
        let credential: Credential = serde_json::from_str(&text).ok()?;
        (credential.api == api_base()).then_some(credential)
    }

    fn save(&self) -> std::io::Result<()> {
        let path = credential_path();
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        // Made with its final mode, so the token is never readable by anyone else.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path)?.write_all(text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn forget() {
        let _ = std::fs::remove_file(credential_path());
    }
}

/// The token requests are made with: `RVRS_TOKEN` first, then what `rvrs login` stored. Nothing
/// sends a run yet, so nothing calls this until runs are kept in the account.
#[allow(dead_code)]
pub fn token() -> Option<String> {
    std::env::var("RVRS_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Credential::stored().map(|credential| credential.token))
}

/// An answer from the API: its status and its body as JSON, or why there is none.
struct Answer {
    status: u16,
    body: serde_json::Value,
}

fn request(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&serde_json::Value>,
    seconds: u64,
) -> Result<Answer, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        .user_agent(format!("rvrs/{VERSION}"))
        // A 4xx is an answer to read, with the reason in its body.
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{}{path}", api_base());
    let unreachable =
        |error: ureq::Error| format!("Couldn't reach the rvrs API at {} ({error}).", api_base());
    let bearer = token.map(|token| format!("Bearer {token}"));
    let mut response = match (method, body) {
        ("POST", Some(body)) => {
            let mut call = agent.post(&url).header("content-type", "application/json");
            if let Some(bearer) = &bearer {
                call = call.header("authorization", bearer);
            }
            call.send(body.to_string()).map_err(unreachable)?
        }
        ("DELETE", _) => {
            let mut call = agent.delete(&url);
            if let Some(bearer) = &bearer {
                call = call.header("authorization", bearer);
            }
            call.call().map_err(unreachable)?
        }
        _ => {
            let mut call = agent.get(&url);
            if let Some(bearer) = &bearer {
                call = call.header("authorization", bearer);
            }
            call.call().map_err(unreachable)?
        }
    };
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    Ok(Answer { status, body })
}

/// The sentence of an error envelope, or one that names the status when there is none.
fn error_sentence(answer: &Answer) -> String {
    answer.body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("The rvrs API answered {}.", answer.status))
}

/// Who a token belongs to, as the API says.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub email: String,
    pub workspace: String,
    pub install: String,
    pub plan: String,
}

fn account_of(body: &serde_json::Value) -> Option<Account> {
    Some(Account {
        email: body["user"]["email"].as_str()?.to_string(),
        workspace: body["workspace"]["name"].as_str()?.to_string(),
        install: body["install"]["name"].as_str()?.to_string(),
        plan: body["plan"]["name"].as_str().unwrap_or("").to_string(),
    })
}

/// Asks the API who this install is. `Ok(None)` is a token the API no longer knows.
pub fn whoami(token: &str, seconds: u64) -> Result<Option<Account>, String> {
    let answer = request("GET", "/v1/installs/me", Some(token), None, seconds)?;
    match answer.status {
        200 => Ok(account_of(&answer.body)),
        401 => Ok(None),
        _ => Err(error_sentence(&answer)),
    }
}

/// What this device calls itself, for the person who confirms the login.
fn device_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is live and its length is passed with it.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    let name = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
    if status == 0 && !name.is_empty() {
        name
    } else {
        "this device".to_string()
    }
}

fn open_in_browser(url: &str) -> bool {
    let tool = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(tool)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn fail(message: &str) -> ExitCode {
    eprintln!(
        "{}",
        crate::trace::style_error(Style::detect_stderr(), message)
    );
    ExitCode::from(1)
}

/// `rvrs login`: start a login, show the code, wait for the person, keep the credential.
pub fn login(arguments: &[String]) -> ExitCode {
    let mut browser = true;
    for argument in arguments {
        match argument.as_str() {
            "--no-browser" => browser = false,
            other => {
                eprintln!("rvrs: rvrs login has no flag {other}. It takes --no-browser.");
                return ExitCode::from(2);
            }
        }
    }
    if std::env::var("RVRS_TOKEN").is_ok_and(|value| !value.trim().is_empty()) {
        return fail("RVRS_TOKEN is set, so this install signs in with that token. Unset it to sign in with rvrs login.");
    }
    if let Some(credential) = Credential::stored() {
        match whoami(&credential.token, 8) {
            Ok(Some(account)) => {
                println!(
                    "Already signed in as {} \u{00B7} {}. rvrs logout signs out.",
                    account.email, account.workspace
                );
                return ExitCode::SUCCESS;
            }
            // A credential the API no longer knows is in the way of a new one.
            Ok(None) => Credential::forget(),
            Err(message) => return fail(&message),
        }
    }

    let style = Style::detect(ColorChoice::Auto);
    let input = serde_json::json!({
        "product": "cli",
        "name": device_name(),
        "version": VERSION,
    });
    let started = match request("POST", "/v1/logins", None, Some(&input), 10) {
        Ok(answer) if answer.status == 201 => answer.body,
        Ok(answer) => return fail(&error_sentence(&answer)),
        Err(message) => return fail(&message),
    };
    let (Some(device_code), Some(user_code), Some(link)) = (
        started["deviceCode"].as_str(),
        started["userCode"].as_str(),
        started["verificationUriComplete"].as_str(),
    ) else {
        return fail("The rvrs API started a login without its codes. Update rvrs and try again.");
    };
    let interval = Duration::from_secs(started["interval"].as_u64().unwrap_or(3).clamp(1, 30));
    let expires = Duration::from_secs(started["expiresIn"].as_u64().unwrap_or(600));

    println!("{} {}\n", style.bold("//"), style.dim("login"));
    println!("  {}  {}", style.dim("Code"), style.bold(user_code));
    println!("  {}  {link}\n", style.dim("Open"));
    let opened = browser && std::io::stdout().is_terminal() && open_in_browser(link);
    println!(
        "{}",
        style.dim(if opened {
            "Waiting for you to confirm the code in the browser that opened."
        } else {
            "Waiting for you to open the link, sign in and confirm the code."
        })
    );

    term::catch_interrupts();
    let deadline = Instant::now() + expires;
    let body = serde_json::json!({ "deviceCode": device_code });
    let mut pause = interval;
    loop {
        let wake = Instant::now() + pause;
        while Instant::now() < wake {
            if term::interrupts() > 0 {
                println!("Stopped. Nothing was signed in.");
                return ExitCode::from(130);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if Instant::now() >= deadline {
            return fail("The code expired before it was confirmed. Run rvrs login again.");
        }
        let answer = match request("POST", "/v1/installs", None, Some(&body), 10) {
            Ok(answer) => answer,
            // One lost request is no reason to give up on a login that takes minutes.
            Err(_) => continue,
        };
        match answer.status {
            202 => {}
            // Asked too fast: back off, as the device grant's slow_down asks.
            429 => pause += Duration::from_secs(5),
            201 => {
                let (Some(token), Some(account)) =
                    (answer.body["token"].as_str(), account_of(&answer.body))
                else {
                    return fail("The rvrs API confirmed the login without a credential. Run rvrs login again.");
                };
                let credential = Credential {
                    api: api_base(),
                    token: token.to_string(),
                    email: account.email.clone(),
                    workspace: account.workspace.clone(),
                    install: account.install,
                };
                if let Err(error) = credential.save() {
                    return fail(&format!(
                        "Signed in, but couldn't keep the credential in {} ({error}).",
                        paths::display(&credential_path())
                    ));
                }
                println!(
                    "{} {} \u{00B7} {}",
                    style.bold_green("Signed in as"),
                    account.email,
                    account.workspace
                );
                return ExitCode::SUCCESS;
            }
            _ => return fail(&error_sentence(&answer)),
        }
    }
}

/// `rvrs logout`: revoke this install's credential at the API and forget it here.
pub fn logout(arguments: &[String]) -> ExitCode {
    if let Some(other) = arguments.first() {
        eprintln!("rvrs: rvrs logout takes nothing, and \u{201C}{other}\u{201D} is something.");
        return ExitCode::from(2);
    }
    let Some(credential) = Credential::stored() else {
        if std::env::var("RVRS_TOKEN").is_ok_and(|value| !value.trim().is_empty()) {
            println!("This install signs in with RVRS_TOKEN. Unset it, and revoke the token in the web app.");
        } else {
            println!("Not signed in.");
        }
        return ExitCode::SUCCESS;
    };
    // The credential goes from this device whatever the API says, so signing out always works.
    let revoked = request(
        "DELETE",
        "/v1/installs/me",
        Some(&credential.token),
        None,
        8,
    );
    Credential::forget();
    match revoked {
        Ok(answer) if answer.status == 204 || answer.status == 401 => {
            println!("Signed out {}.", credential.email);
        }
        _ => println!(
            "Signed out {} on this device. The API couldn't be reached, so revoke \u{201C}{}\u{201D} in the web app too.",
            credential.email, credential.install
        ),
    }
    ExitCode::SUCCESS
}

/// The Account line of `rvrs status`: who is signed in, from what `rvrs login` stored or, for a
/// token from the environment, from the API.
pub fn status_line() -> (bool, String) {
    if let Ok(value) = std::env::var("RVRS_TOKEN") {
        if !value.trim().is_empty() {
            return match whoami(value.trim(), 4) {
                Ok(Some(account)) => (
                    true,
                    format!(
                        "signed in as {} \u{00B7} {} \u{00B7} RVRS_TOKEN",
                        account.email, account.workspace
                    ),
                ),
                Ok(None) => (
                    false,
                    "RVRS_TOKEN is not a credential the API knows".to_string(),
                ),
                Err(_) => (
                    false,
                    "RVRS_TOKEN is set \u{00B7} the API can't be reached".to_string(),
                ),
            };
        }
    }
    match Credential::stored() {
        Some(credential) => (
            true,
            format!(
                "signed in as {} \u{00B7} {}",
                credential.email, credential.workspace
            ),
        ),
        None => (
            false,
            "not signed in \u{00B7} rvrs login signs in".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_read_from_the_apis_answer() {
        let body = serde_json::json!({
            "install": { "id": "x", "name": "laptop.local", "product": "cli" },
            "user": { "id": "u", "email": "ada@example.com", "name": "Ada" },
            "workspace": { "id": "w", "name": "Ada\u{2019}s workspace", "slug": "ada", "role": "owner" },
            "plan": { "key": "free", "name": "Free" },
        });
        assert_eq!(
            account_of(&body),
            Some(Account {
                email: "ada@example.com".into(),
                workspace: "Ada\u{2019}s workspace".into(),
                install: "laptop.local".into(),
                plan: "Free".into(),
            })
        );
        assert_eq!(account_of(&serde_json::json!({ "user": {} })), None);
    }

    #[test]
    fn an_error_is_the_envelopes_sentence_or_names_the_status() {
        let envelope = Answer {
            status: 410,
            body: serde_json::json!({ "error": { "code": "LOGIN_EXPIRED", "message": "The login expired or was used already. Start a new one." } }),
        };
        assert_eq!(
            error_sentence(&envelope),
            "The login expired or was used already. Start a new one."
        );
        let bare = Answer {
            status: 502,
            body: serde_json::Value::Null,
        };
        assert_eq!(error_sentence(&bare), "The rvrs API answered 502.");
    }

    #[test]
    fn a_credential_reads_back_as_it_was_written() {
        let credential = Credential {
            api: "https://api.rvrs.dev".into(),
            token: "rvrs_test".into(),
            email: "ada@example.com".into(),
            workspace: "Ada\u{2019}s workspace".into(),
            install: "laptop.local".into(),
        };
        let text = serde_json::to_string(&credential).unwrap();
        assert_eq!(
            serde_json::from_str::<Credential>(&text).unwrap(),
            credential
        );
    }
}
