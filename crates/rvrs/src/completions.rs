//! Tab completion for zsh, bash and fish. The scripts ask the binary for targets and run ids
//! from history, so `rvrs traceroute <tab>` offers what was traced before.

use std::path::PathBuf;

pub const ZSH: &str = r#"#compdef rvrs

_rvrs_targets() {
  local -a targets
  targets=(${(f)"$(rvrs __targets 2>/dev/null)"})
  targets=(${targets//:/\\:})
  (( $#targets )) && _describe -t targets 'recent target' targets
  _hosts
}

_rvrs_runs() {
  local -a runs
  runs=(${(f)"$(rvrs __runs 2>/dev/null)"})
  (( $#runs )) && _describe -t runs 'run' runs
}

_rvrs_cases() {
  local -a cases
  cases=(${(f)"$(rvrs __cases 2>/dev/null)"})
  cases=(${cases//:/\\:})
  (( $#cases )) && _describe -t cases 'case' cases
}

_rvrs_case() {
  local -a actions
  actions=(
    'new:Start a case'
    'rename:Rename a case'
    'delete:Delete a case; its runs stay saved'
    'add:Put a run in a case'
    'remove:Take a run out of a case'
  )
  case $CURRENT in
    3)
      _describe -t actions 'action' actions
      _rvrs_cases
      ;;
    4)
      case $words[3] in
        rename|delete|add|remove) _rvrs_cases ;;
      esac
      ;;
    5)
      case $words[3] in
        add|remove) _rvrs_runs ;;
      esac
      ;;
  esac
}

_rvrs() {
  local -a commands
  commands=(
    'traceroute:The path to a target, hop by hop'
    'ping:Latency and loss to a target, packet by packet'
    'mtr:Loss and latency at every hop, round after round'
    'dig:One name asked of every source, with the answers compared'
    'bgp:The route an address falls in, its origin validation and its network'
    'last:Show the newest run again'
    'show:Show a run from history'
    'history:List the runs kept on this device'
    'save:Save the newest run, or the one named'
    'saved:List the saved runs'
    'case:List the cases, or the runs in one'
    'cases:List the cases'
    'favorite:Keep a host in Favorites'
    'favorites:List the favorite hosts'
    'login:Sign this install in to your workspace'
    'logout:Sign out and revoke this install'\''s credential'
    'sync:Send runs to the account and read what is new'
    'share:Print the share link of a run, a public page'
    'status:Check that this device can run the tools'
    'update:Bring rvrs up to date'
    'completions:Set up tab completion'
    'help:Show help'
  )
  if (( CURRENT == 2 )); then
    _describe -t commands 'command' commands
    _rvrs_targets
    return
  fi
  case $words[2] in
    traceroute)
      _arguments -s \
        '(-m --max-hops)'{-m,--max-hops}'[last hop to probe]:hops:' \
        '(-q --queries)'{-q,--queries}'[probes per hop]:probes:' \
        '(-w --wait)'{-w,--wait}'[longest wait for a reply, seconds]:seconds:' \
        '(-f --first)'{-f,--first}'[first hop to probe]:hop:' \
        '(-z --sendwait)'{-z,--sendwait}'[pause between probes]:pause:' \
        '-n[addresses only, no reverse DNS]' \
        '(-A --as-path-lookups)'{-A,--as-path-lookups}'[show AS numbers in the classic output]' \
        '-4[resolve the target to IPv4]' \
        '-6[resolve the target to IPv6]' \
        '(-I --icmp)'{-I,--icmp}'[ICMP echo probes]' \
        '(-T --tcp)'{-T,--tcp}'[TCP probes to a port]' \
        '(-p --port)'{-p,--port}'[the port for -T]:port:' \
        '--mtu[find the path MTU]' \
        '--json[print the run as a report]' \
        '--classic[print what traceroute prints]' \
        '--no-lookups[no reverse DNS and no network lookups]' \
        '--color=[when to use color]:when:(auto always never)' \
        '--node=[run from a node]:node:' \
        '--no-wait[with --node, print the run id and leave]' \
        '--expires-in=[with --node, seconds the request waits]:seconds:' \
        '--case=[save the run into a case]:case:_rvrs_cases' \
        '--help[show help]' \
        '1:target:_rvrs_targets'
      ;;
    mtr)
      _arguments -s \
        '(-c --report-cycles)'{-c,--report-cycles}'[stop after n rounds]:rounds:' \
        '(-r --report)'{-r,--report}'[print the report when the rounds are done]' \
        '(-w --report-wide)'{-w,--report-wide}'[a report that fits the longest name]' \
        '(-i --interval)'{-i,--interval}'[seconds between rounds]:seconds:' \
        '(-m --max-ttl)'{-m,--max-ttl}'[last hop to probe]:hops:' \
        '(-f --first-ttl)'{-f,--first-ttl}'[first hop to probe]:hop:' \
        '(-s --psize)'{-s,--psize}'[packet size in bytes]:bytes:' \
        '(-Z --timeout)'{-Z,--timeout}'[seconds before a probe counts as a loss]:seconds:' \
        '(-n --no-dns)'{-n,--no-dns}'[addresses only, no reverse DNS]' \
        '(-b --show-ips)'{-b,--show-ips}'[names and addresses in the report]' \
        '(-z --aslookup)'{-z,--aslookup}'[AS numbers in the report]' \
        '-4[resolve the target to IPv4]' \
        '-6[resolve the target to IPv6]' \
        '--json[print the run as a report]' \
        '--classic[print what mtr reports]' \
        '--case=[save the run into this case]:case:' \
        '--no-lookups[no reverse DNS and no network lookups]' \
        '--color=[when to use color]:when:(auto always never)' \
        '--help[show help]' \
        '1:target:_rvrs_targets'
      ;;
    dig|dns)
      _arguments -s \
        '-t[the record type to ask for]:type:(A AAAA CNAME MX TXT NS SOA HTTPS CAA DS DNSKEY PTR SRV ANY)' \
        '-x[the name for an address]:address:' \
        '-q[the name to ask for]:name:' \
        '-p[the port of the server]:port:' \
        '-4[ask over IPv4]' \
        '-6[ask over IPv6]' \
        '--resolver=[a resolver of your own, asked beside the public ones]:address:' \
        '--json[print the lookup as a report]' \
        '--classic[print what dig prints]' \
        '--case=[save the lookup into this case]:case:' \
        '--no-lookups[no lookup of the public address of this device]' \
        '--color=[when to use color]:when:(auto always never)' \
        '--help[show help]' \
        '1:name:_rvrs_targets' \
        '*:type or option:(A AAAA CNAME MX TXT NS SOA HTTPS CAA DS DNSKEY PTR SRV ANY +short +trace +tcp +dnssec +norecurse +noall +answer)'
      ;;
    bgp)
      _arguments -s \
        '--paths[every distinct AS path, and what your looking glasses hold]' \
        '--prefixes[the prefixes the network originates]' \
        '--neighbors[the networks seen upstream and downstream of an AS]' \
        '--history[who announced the prefix, its visibility and BGP updates]' \
        '--days=[how far back History looks]:days:(7 30 90)' \
        '--rpki[the origin validation spelled out]' \
        '--sources[where each part came from and when it was observed]' \
        '--all[everything]' \
        '--no-glasses[do not ask your looking glasses]' \
        '--type=[the looking glass type]:type:(alice-lg lg-cgi hyperglass bird-lg-go)' \
        '--name=[what to call the looking glass]:name:' \
        '--router=[a router of the looking glass to ask]:router:' \
        '--raw[with test, print the router output]' \
        '--json[print the lookup as a report]' \
        '--classic[print plain text with its sources]' \
        '--case=[save the lookup into this case]:case:' \
        '--no-lookups[no lookup of the network this device is in]' \
        '--color=[when to use color]:when:(auto always never)' \
        '--help[show help]' \
        '1:target or command:(check glass)' \
        '*:target:_rvrs_targets'
      ;;
    ping)
      _arguments -s \
        '-c[stop after n packets]:packets:' \
        '-i[seconds between packets]:seconds:' \
        '-W[seconds before a packet counts as a loss]:seconds:' \
        '-w[stop after this many seconds]:seconds:' \
        '-s[payload size in bytes]:bytes:' \
        '-t[TTL of the packets sent]:ttl:' \
        '-n[addresses only, no reverse DNS]' \
        '-q[only the opening line and the statistics]' \
        '-4[resolve the target to IPv4]' \
        '-6[resolve the target to IPv6]' \
        '--json[print the run as a report]' \
        '--classic[print what ping prints]' \
        '--no-lookups[no reverse DNS and no network lookup]' \
        '--color=[when to use color]:when:(auto always never)' \
        '--node=[run from a node]:node:' \
        '--no-wait[with --node, print the run id and leave]' \
        '--expires-in=[with --node, seconds the request waits]:seconds:' \
        '--case=[save the run into a case]:case:_rvrs_cases' \
        '--help[show help]' \
        '1:target:_rvrs_targets'
      ;;
    show)
      _arguments '--json[print the run as a report]' '--classic[print what the original tool prints]' '1:run:_rvrs_runs'
      ;;
    last)
      _arguments '--json[print the run as a report]' '--classic[print what the original tool prints]'
      ;;
    history)
      _arguments '--json[print the reports]' '*:tool, target or clear:(clear traceroute ping mtr dig bgp)'
      ;;
    save)
      _arguments '--case=[save into this case]:case:_rvrs_cases' '1:run:_rvrs_runs'
      ;;
    share)
      _arguments \
        '--no-private-networks[leave the private network out of the page]' \
        '--public-address[show the public address on the page]' \
        '--install-name[show the name of the install that made the run]' \
        '--stop[end the link]' \
        '--json[print the link with what it shows]' \
        '1:run:_rvrs_runs'
      ;;
    favorite)
      _arguments '1:host:_rvrs_targets'
      ;;
    favorites)
      _arguments '1:action:(remove)' '2:host:_rvrs_targets'
      ;;
    saved)
      _arguments '--json[print the reports]' '1:action:(remove)' '2:run:_rvrs_runs'
      ;;
    case|cases)
      _rvrs_case
      ;;
    completions)
      _arguments '1:shell:(install zsh bash fish)'
      ;;
  esac
}

if [ "$funcstack[1]" = "_rvrs" ]; then
  _rvrs "$@"
else
  compdef _rvrs rvrs
fi
"#;

pub const BASH: &str = r#"_rvrs() {
  local current previous command
  current="${COMP_WORDS[COMP_CWORD]}"
  previous="${COMP_WORDS[COMP_CWORD-1]}"
  command="${COMP_WORDS[1]}"
  if [ "$COMP_CWORD" -eq 1 ]; then
    COMPREPLY=($(compgen -W "traceroute ping mtr dig bgp last show history save saved case cases favorite favorites login logout sync share status update completions help $(rvrs __targets 2>/dev/null)" -- "$current"))
    return
  fi
  case "$command" in
    traceroute)
      if [[ "$current" == -* ]]; then
        COMPREPLY=($(compgen -W "-m -q -w -f -z -n -A -4 -6 -I -T -p --tcp --port --mtu --max-hops --queries --wait --first --json --classic --no-lookups --color --node --no-wait --expires-in --case --help" -- "$current"))
      elif [ "$previous" = "--case" ]; then
        local IFS=$'\n'
        COMPREPLY=($(compgen -W "$(rvrs __cases 2>/dev/null)" -- "$current"))
      elif [ "$previous" = "--color" ]; then
        COMPREPLY=($(compgen -W "auto always never" -- "$current"))
      else
        COMPREPLY=($(compgen -W "$(rvrs __targets 2>/dev/null)" -- "$current"))
      fi
      ;;
    mtr)
      if [[ "$current" == -* ]]; then
        COMPREPLY=($(compgen -W "-c -r -w -i -m -f -s -Z -n -b -z -4 -6 --report --report-wide --report-cycles --interval --json --classic --case --no-lookups --color --help" -- "$current"))
      else
        COMPREPLY=($(compgen -W "$(rvrs __targets 2>/dev/null)" -- "$current"))
      fi
      ;;
    dig|dns)
      if [[ "$current" == -* ]]; then
        COMPREPLY=($(compgen -W "-t -x -q -p -4 -6 --resolver --json --classic --case --no-lookups --color --help" -- "$current"))
      elif [[ "$current" == +* ]]; then
        COMPREPLY=($(compgen -W "+short +trace +tcp +dnssec +norecurse +noall +answer +authority +additional +nocomments +nostats +nocmd +time= +tries=" -- "$current"))
      elif [ "$previous" = "-t" ]; then
        COMPREPLY=($(compgen -W "A AAAA CNAME MX TXT NS SOA HTTPS CAA DS DNSKEY PTR SRV ANY" -- "$current"))
      else
        COMPREPLY=($(compgen -W "$(rvrs __targets 2>/dev/null)" -- "$current"))
      fi
      ;;
    bgp)
      if [[ "$current" == -* ]]; then
        COMPREPLY=($(compgen -W "--paths --prefixes --neighbors --history --days --rpki --sources --all --no-glasses --type --name --router --raw --json --classic --case --no-lookups --color --help" -- "$current"))
      elif [ "$previous" = "--type" ]; then
        COMPREPLY=($(compgen -W "alice-lg lg-cgi hyperglass bird-lg-go" -- "$current"))
      elif [ "$previous" = "--days" ]; then
        COMPREPLY=($(compgen -W "7 30 90" -- "$current"))
      elif [ "$previous" = "glass" ]; then
        COMPREPLY=($(compgen -W "add remove test" -- "$current"))
      else
        COMPREPLY=($(compgen -W "check glass $(rvrs __targets 2>/dev/null)" -- "$current"))
      fi
      ;;
    ping)
      if [[ "$current" == -* ]]; then
        COMPREPLY=($(compgen -W "-c -i -W -w -s -t -n -q -4 -6 --json --classic --no-lookups --color --node --no-wait --expires-in --case --help" -- "$current"))
      elif [ "$previous" = "--case" ]; then
        local IFS=$'\n'
        COMPREPLY=($(compgen -W "$(rvrs __cases 2>/dev/null)" -- "$current"))
      else
        COMPREPLY=($(compgen -W "$(rvrs __targets 2>/dev/null)" -- "$current"))
      fi
      ;;
    show)
      COMPREPLY=($(compgen -W "--json --classic $(rvrs __runs 2>/dev/null | cut -d: -f1)" -- "$current"))
      ;;
    last)
      COMPREPLY=($(compgen -W "--json --classic" -- "$current"))
      ;;
    favorite|favorites)
      COMPREPLY=($(compgen -W "remove $(rvrs __targets 2>/dev/null)" -- "$current"))
      ;;
    save)
      if [ "$previous" = "--case" ]; then
        local IFS=$'\n'
        COMPREPLY=($(compgen -W "$(rvrs __cases 2>/dev/null)" -- "$current"))
      else
        COMPREPLY=($(compgen -W "--case $(rvrs __runs 2>/dev/null | cut -d: -f1)" -- "$current"))
      fi
      ;;
    saved)
      COMPREPLY=($(compgen -W "remove --json $(rvrs __runs 2>/dev/null | cut -d: -f1)" -- "$current"))
      ;;
    share)
      COMPREPLY=($(compgen -W "--no-private-networks --public-address --install-name --stop --json $(rvrs __runs 2>/dev/null | cut -d: -f1)" -- "$current"))
      ;;
    case|cases)
      if [ "$COMP_CWORD" -eq 2 ]; then
        local IFS=$'\n'
        COMPREPLY=($(compgen -W "$(printf '%s\n' new rename delete add remove --json; rvrs __cases 2>/dev/null)" -- "$current"))
      elif [ "$COMP_CWORD" -eq 3 ] && [ "$previous" != "new" ]; then
        local IFS=$'\n'
        COMPREPLY=($(compgen -W "$(rvrs __cases 2>/dev/null)" -- "$current"))
      elif [ "$COMP_CWORD" -eq 4 ]; then
        case "${COMP_WORDS[2]}" in
          add|remove) COMPREPLY=($(compgen -W "$(rvrs __runs 2>/dev/null | cut -d: -f1)" -- "$current")) ;;
        esac
      fi
      ;;
    history)
      COMPREPLY=($(compgen -W "clear --json traceroute ping mtr dig bgp $(rvrs __targets 2>/dev/null)" -- "$current"))
      ;;
    completions)
      COMPREPLY=($(compgen -W "install zsh bash fish" -- "$current"))
      ;;
  esac
}
complete -F _rvrs rvrs
"#;

pub const FISH: &str = r#"complete -c rvrs -f
complete -c rvrs -n __fish_use_subcommand -a traceroute -d 'The path to a target, hop by hop'
complete -c rvrs -n __fish_use_subcommand -a ping -d 'Latency and loss to a target, packet by packet'
complete -c rvrs -n __fish_use_subcommand -a mtr -d 'Loss and latency at every hop, round after round'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s c -l report-cycles -x -d 'Stop after n rounds'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s r -l report -d 'Print the report when the rounds are done'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s w -l report-wide -d 'A report that fits the longest name'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s i -l interval -x -d 'Seconds between rounds'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s m -l max-ttl -x -d 'Last hop to probe'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s n -l no-dns -d 'Addresses only, no reverse DNS'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s b -l show-ips -d 'Names and addresses in the report'
complete -c rvrs -n '__fish_seen_subcommand_from mtr' -s z -l aslookup -d 'AS numbers in the report'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s c -x -d 'Stop after n packets'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s i -x -d 'Seconds between packets'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s W -x -d 'Seconds before a packet counts as a loss'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s w -x -d 'Stop after this many seconds'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s s -x -d 'Payload size in bytes'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s t -x -d 'TTL of the packets sent'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s q -d 'Only the opening line and the statistics'
complete -c rvrs -n '__fish_seen_subcommand_from ping' -s n -d 'Addresses only, no reverse DNS'
complete -c rvrs -n __fish_use_subcommand -a dig -d 'One name asked of every source, with the answers compared'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -a 'A AAAA CNAME MX TXT NS SOA HTTPS CAA' -d 'Record type'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -a '+short +trace +tcp +dnssec +norecurse' -d 'dig option'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -s t -x -d 'The record type to ask for'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -s x -x -d 'The name for an address'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -s p -x -d 'The port of the server'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -l resolver -x -d 'A resolver of your own, asked beside the public ones'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -l json -d 'Print the lookup as a report'
complete -c rvrs -n '__fish_seen_subcommand_from dig dns' -l classic -d 'Print what dig prints'
complete -c rvrs -n __fish_use_subcommand -a bgp -d 'The route an address falls in, its origin validation and its network'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -a 'check' -d 'Would this route pass RPKI origin validation?'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -a 'glass' -d 'Your looking glasses'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l paths -d 'Every distinct AS path, and what your looking glasses hold'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l prefixes -d 'The prefixes the network originates'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l neighbors -d 'The networks seen upstream and downstream of an AS'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l history -d 'Who announced the prefix, its visibility and BGP updates'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l days -x -a '7 30 90' -d 'How far back History looks'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l rpki -d 'The origin validation spelled out'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l sources -d 'Where each part came from and when it was observed'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l all -d 'Everything'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l no-glasses -d 'Do not ask your looking glasses'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l type -x -a 'alice-lg lg-cgi hyperglass bird-lg-go' -d 'The looking glass type'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l router -x -d 'A router of the looking glass to ask'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l json -d 'Print the lookup as a report'
complete -c rvrs -n '__fish_seen_subcommand_from bgp' -l classic -d 'Print plain text with its sources'
complete -c rvrs -n __fish_use_subcommand -a last -d 'Show the newest run again'
complete -c rvrs -n __fish_use_subcommand -a show -d 'Show a run from history'
complete -c rvrs -n __fish_use_subcommand -a history -d 'List the runs kept on this device'
complete -c rvrs -n __fish_use_subcommand -a save -d 'Save the newest run, or the one named'
complete -c rvrs -n __fish_use_subcommand -a saved -d 'List the saved runs'
complete -c rvrs -n __fish_use_subcommand -a case -d 'List the cases, or the runs in one'
complete -c rvrs -n __fish_use_subcommand -a cases -d 'List the cases'
complete -c rvrs -n __fish_use_subcommand -a favorite -d 'Keep a host in Favorites'
complete -c rvrs -n __fish_use_subcommand -a favorites -d 'List the favorite hosts'
complete -c rvrs -n '__fish_seen_subcommand_from favorite favorites' -a '(rvrs __targets 2>/dev/null)'
complete -c rvrs -n '__fish_seen_subcommand_from save' -a '(rvrs __runs 2>/dev/null | string replace : \t)'
complete -c rvrs -n __fish_use_subcommand -a share -d 'Print the share link of a run, a public page'
complete -c rvrs -n '__fish_seen_subcommand_from share' -a '(rvrs __runs 2>/dev/null | string replace : \t)'
complete -c rvrs -n '__fish_seen_subcommand_from share' -l no-private-networks -d 'Leave the private network out of the page'
complete -c rvrs -n '__fish_seen_subcommand_from share' -l public-address -d 'Show the public address on the page'
complete -c rvrs -n '__fish_seen_subcommand_from share' -l install-name -d 'Show the name of the install that made the run'
complete -c rvrs -n '__fish_seen_subcommand_from share' -l stop -d 'End the link'
complete -c rvrs -n '__fish_seen_subcommand_from share' -l json -d 'Print the link with what it shows'
complete -c rvrs -n '__fish_seen_subcommand_from saved' -a remove -d 'Remove a saved run and its place in every case'
complete -c rvrs -n '__fish_seen_subcommand_from saved' -a '(rvrs __runs 2>/dev/null | string replace : \t)'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new rename delete add remove' -a 'new' -d 'Start a case'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new rename delete add remove' -a 'rename' -d 'Rename a case'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new rename delete add remove' -a 'delete' -d 'Delete a case; its runs stay saved'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new rename delete add remove' -a 'add' -d 'Put a run in a case'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new rename delete add remove' -a 'remove' -d 'Take a run out of a case'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and not __fish_seen_subcommand_from new' -a '(rvrs __cases 2>/dev/null)' -d 'Case'
complete -c rvrs -n '__fish_seen_subcommand_from case cases; and __fish_seen_subcommand_from add remove' -a '(rvrs __runs 2>/dev/null | string replace : \t)'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute ping save' -l case -x -a '(rvrs __cases 2>/dev/null)' -d 'Save the run into a case'
complete -c rvrs -n __fish_use_subcommand -a status -d 'Check that this device can run the tools'
complete -c rvrs -n __fish_use_subcommand -a update -d 'Bring rvrs up to date'
complete -c rvrs -n '__fish_seen_subcommand_from update' -l check -d 'Only say whether a newer version is out'
complete -c rvrs -n __fish_use_subcommand -a completions -d 'Set up tab completion'
complete -c rvrs -n __fish_use_subcommand -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -a '(rvrs __targets 2>/dev/null)' -d 'Recent target'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s m -l max-hops -x -d 'Last hop to probe'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s q -l queries -x -d 'Probes per hop'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s w -l wait -x -d 'Longest wait for a reply, seconds'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s f -l first -x -d 'First hop to probe'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s z -l sendwait -x -d 'Pause between probes'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s n -d 'Addresses only, no reverse DNS'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s A -l as-path-lookups -d 'Show AS numbers in the classic output'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s 4 -d 'Resolve the target to IPv4'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -s 6 -d 'Resolve the target to IPv6'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -l no-lookups -d 'No reverse DNS and no network lookups'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute ping last show saved case cases' -l json -d 'Print the run as a report'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute ping last show' -l classic -d 'Print what the original tool prints'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -l color -x -a 'auto always never' -d 'When to use color'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -l node -x -d 'Run from a node'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -l no-wait -d 'With --node, print the run id and leave'
complete -c rvrs -n '__fish_seen_subcommand_from traceroute' -l expires-in -x -d 'With --node, seconds the request waits'
complete -c rvrs -n '__fish_seen_subcommand_from show' -a '(rvrs __runs 2>/dev/null | string replace : \t)'
complete -c rvrs -n '__fish_seen_subcommand_from history' -a clear -d 'Delete every run in history'
complete -c rvrs -n '__fish_seen_subcommand_from history' -a 'traceroute ping mtr dig bgp' -d 'Only this tool'
complete -c rvrs -n '__fish_seen_subcommand_from completions' -a 'install zsh bash fish'
"#;

pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "zsh" => Some(ZSH),
        "bash" => Some(BASH),
        "fish" => Some(FISH),
        _ => None,
    }
}

/// Where `rvrs completions install` writes for a shell, and what the person still has to do,
/// if anything. fish and bash-completion load from their folder by themselves.
pub fn install_target(shell: &str) -> Option<(PathBuf, Option<String>)> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let xdg = |variable: &str, fallback: &str| {
        std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(fallback), PathBuf::from)
    };
    match shell {
        "fish" => Some((
            xdg("XDG_CONFIG_HOME", ".config").join("fish/completions/rvrs.fish"),
            None,
        )),
        "bash" => Some((
            xdg("XDG_DATA_HOME", ".local/share").join("bash-completion/completions/rvrs"),
            None,
        )),
        "zsh" => {
            // Homebrew's site-functions folder is on zsh's fpath wherever Homebrew is set up.
            for dir in [
                "/opt/homebrew/share/zsh/site-functions",
                "/usr/local/share/zsh/site-functions",
            ] {
                let dir = PathBuf::from(dir);
                let writable = std::fs::metadata(&dir)
                    .is_ok_and(|meta| meta.is_dir() && !meta.permissions().readonly())
                    && std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(dir.join("_rvrs"))
                        .is_ok();
                if writable {
                    return Some((dir.join("_rvrs"), None));
                }
            }
            let note =
                "Add this line to ~/.zshrc before compinit: fpath=(~/.zfunc $fpath)".to_string();
            Some((home.join(".zfunc/_rvrs"), Some(note)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_has_a_script_that_asks_for_history() {
        for shell in ["zsh", "bash", "fish"] {
            let script = script(shell).unwrap();
            assert!(script.contains("rvrs __targets"), "{shell}");
            assert!(script.contains("rvrs __cases"), "{shell}");
            assert!(
                script.contains("saved") && script.contains("rvrs __cases"),
                "{shell}"
            );
            assert!(script.contains("traceroute"), "{shell}");
            assert!(script.contains("+trace"), "{shell}");
            assert!(
                script.contains("bgp") && script.contains("bird-lg-go"),
                "{shell}"
            );
        }
        assert!(script("powershell").is_none());
    }
}
