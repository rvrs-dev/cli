//! The run picker: `r` on a run in a list, or on a host in a result, opens it over the screen
//! with every tool that can take that host. Return runs the selected tool, and a tool's letter
//! runs it at once. `R` in a result runs the open run again without it.

use rvrs_core::bgp::Target;

use super::*;

/// The picker while it is open: the host, the tools that can take it, and the selected one.
pub(super) struct Launcher {
    pub(super) target: String,
    pub(super) tools: Vec<Tool>,
    pub(super) row: usize,
}

/// The tools that can take a host, in the Console's order. An AS number or a prefix only goes
/// to BGP. An address has no DNS lookup, and a private one has no public route.
pub(super) fn tools_for(target: &str) -> Vec<Tool> {
    if Target::is_routing_only(target) {
        return [Tool::Bgp]
            .into_iter()
            .filter(|tool| crate::features::runs(*tool))
            .collect();
    }
    let host = normalize_target(target);
    let mut tools = vec![Tool::Traceroute, Tool::Mtr, Tool::Ping];
    match host.parse::<IpAddr>() {
        Ok(address) if is_private(address) => {}
        Ok(_) => tools.push(Tool::Bgp),
        Err(_) => tools.extend([Tool::Dns, Tool::Bgp]),
    }
    // Only the tools this build runs.
    tools.retain(|tool| crate::features::runs(*tool));
    tools
}

/// The letter that runs a tool from the picker.
pub(super) fn letter(tool: Tool) -> char {
    match tool {
        Tool::Traceroute => 't',
        Tool::Mtr => 'm',
        Tool::Ping => 'p',
        Tool::Dns => 'd',
        Tool::Bgp => 'b',
        Tool::Iperf3 => 'i',
    }
}

impl App {
    /// Opens the picker for a host. `tool` is selected first when the host can take it.
    pub(super) fn open_launcher(&mut self, target: &str, tool: Option<Tool>) {
        let tools = tools_for(target);
        if tools.is_empty() {
            return;
        }
        let row = tool
            .and_then(|tool| tools.iter().position(|known| *known == tool))
            .unwrap_or(0);
        self.launcher = Some(Launcher {
            target: target.to_string(),
            tools,
            row,
        });
    }

    pub(super) fn launcher_key(&mut self, key: KeyEvent) {
        let Some(launcher) = &mut self.launcher else {
            return;
        };
        let chosen = match key.code {
            KeyCode::Esc | KeyCode::Left => {
                self.launcher = None;
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                launcher.row = (launcher.row + 1).min(launcher.tools.len() - 1);
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                launcher.row = launcher.row.saturating_sub(1);
                return;
            }
            KeyCode::Enter | KeyCode::Right => launcher.tools.get(launcher.row).copied(),
            KeyCode::Char(typed) => launcher
                .tools
                .iter()
                .copied()
                .find(|tool| letter(*tool) == typed),
            _ => None,
        };
        let Some(tool) = chosen else { return };
        let target = launcher.target.clone();
        self.launcher = None;
        self.tool = tool;
        self.start_with(tool, &target);
    }

    /// The host a result's selection names, for the picker: the selected hop, the host a
    /// record names, or the network, prefix or address of a BGP row. The run's target when
    /// nothing is selected.
    pub(super) fn selected_result_host(&self) -> Option<String> {
        let run = self.run()?;
        let selected = match run.tool {
            Tool::Dns => self.dns_record().as_ref().and_then(tool_host),
            Tool::Bgp => self.bgp_selected_host(),
            Tool::Ping | Tool::Iperf3 => None,
            Tool::Traceroute | Tool::Mtr => run.path().get(self.hop).and_then(|hop| {
                let address = hop.address()?;
                Some(
                    run.name(address)
                        .map_or_else(|| address.to_string(), str::to_string),
                )
            }),
        };
        Some(selected.unwrap_or_else(|| run.target.clone()))
    }

    /// The picker, over the middle of the screen.
    pub(super) fn draw_launcher(&self, frame: &mut Frame, area: Rect) {
        let Some(launcher) = &self.launcher else {
            return;
        };
        let box_width = 36.min(area.width.saturating_sub(4));
        let box_height = (launcher.tools.len() as u16 + 4).min(area.height.saturating_sub(2));
        let rect = Rect {
            x: area.x + (area.width - box_width) / 2,
            y: area.y + (area.height - box_height) / 2,
            width: box_width,
            height: box_height,
        };
        let count = launcher.tools.len();
        let block = panel(
            "Run",
            &truncate_middle(&launcher.target, usize::from(box_width).saturating_sub(16)),
            &format!("{count} {}", if count == 1 { "tool" } else { "tools" }),
        );
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let columns = usize::from(inner.width);
        let mut lines = vec![Line::default()];
        for (index, tool) in launcher.tools.iter().enumerate() {
            let key = format!(" <{}>  ", letter(*tool));
            let name = tool.command().to_string();
            if index == launcher.row {
                lines.push(Line::styled(
                    fit(&format!("{key}{name}"), columns),
                    selected(),
                ));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(key, Paint::new().fg(KEY).add_modifier(Modifier::BOLD)),
                    Span::styled(name, Paint::new().add_modifier(Modifier::BOLD)),
                ]));
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
