//! Command-line interface for beemr.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use beemr::config::{default_device_name, Config};
use beemr::discovery::Dht;
use beemr::get::GetOptions;
use beemr::identity::{Contacts, DeviceId};
use beemr::message::{self, OutboxEntry};
use beemr::node::{Mode, Node, NodeOptions};
use beemr::profile::Profile;
use beemr::share::ShareOptions;
use beemr::{daemon, doctor, relay, service, util, Error, Result};

const USAGE: &str = "\
beemr: send files and messages straight to another device.
No servers, no accounts, no setup. Everything is end-to-end encrypted.

Files:
  beemr share <file-or-folder> [options]   Share it and print a command for the other device
  beemr get <ticket> [-o <folder>]         Download something shared with you

Messages:
  beemr msg <contact-or-id> <message>      Send a message
  beemr inbox [--all]                      Read your messages
  beemr reply <number> <message>           Reply to a message in your inbox

This device:
  beemr setup                              Name this device and start the background service
  beemr id                                 Show this device's ID (others use it to reach you)
  beemr name [new name]                    Show or change this device's name
  beemr contact add <name> <device-id>     Save someone's device under a name
  beemr contact list | remove <name>
  beemr daemon [status|install|uninstall|run]   The background service that receives messages
  beemr relay [run|use <address>|list|remove <address>]   Relay for devices that can't connect directly
  beemr doctor                             Check how reachable this device is

Share options:
  --to <contact-or-id>    Only this device may download (repeat for several)
  -n, --downloads <n>     How many downloads to allow (default 1, 0 = unlimited)
  -e, --expires <time>    Stop accepting downloads after e.g. 30s, 10m, 2h or 1d
  -p, --port <port>       Listen on this port (default: random)
  --no-upnp               Don't ask the router to open a port
";

#[tokio::main]
async fn main() -> ExitCode {
    // Diagnostics for troubleshooting, e.g. BEEMR_LOG=debug or
    // BEEMR_LOG=libp2p_relay=debug,beemr=debug.
    if let Ok(filter) = std::env::var("BEEMR_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stderr)
            .try_init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        print!("{USAGE}");
        return Ok(());
    };
    match command.as_str() {
        "share" | "send" => share(rest).await,
        "get" | "receive" => get(rest).await,
        "msg" | "message" => msg(rest).await,
        "inbox" => inbox(rest),
        "reply" => reply(rest).await,
        "setup" => setup(rest),
        "id" => show_id(),
        "name" => name(rest),
        "contact" | "contacts" => contact(rest),
        "daemon" | "service" => daemon_command(rest).await,
        "relay" => relay_command(rest).await,
        "doctor" => doctor::run(load_profile(true)?).await,
        // Developer tool: a standalone DHT node for private test networks.
        "dht-node" => dht_node().await,
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "version" | "-V" | "--version" => {
            println!("beemr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(Error::new(format!(
            "unknown command '{other}' (run `beemr help` for usage)"
        ))),
    }
}

/// Load this device's profile. On first use, ask for a device name when a
/// person is at the keyboard.
fn load_profile(ask_name: bool) -> Result<Profile> {
    let config = Config::load()?;
    if ask_name && config.device_name()?.is_none() && std::io::stdin().is_terminal() {
        let name = prompt_name(&default_device_name())?;
        config.set_device_name(&name)?;
    }
    let profile = Profile::load_from(config)?;
    if profile.created {
        eprintln!(
            "Created this device's identity. Its ID is:\n\n    {}\n",
            profile.identity.id()
        );
    }
    Ok(profile)
}

fn prompt_name(default: &str) -> Result<String> {
    eprint!("Name this device (others will see it) [{default}]: ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() { default } else { line }.to_string())
}

async fn share(args: &[String]) -> Result<()> {
    let profile = load_profile(true)?;
    let mut options = ShareOptions {
        path: PathBuf::new(),
        allowed: Vec::new(),
        max_downloads: Some(1),
        expires_in: None,
        port: 0,
        upnp: true,
    };
    let mut path = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--to" => options
                .allowed
                .push(profile.contacts.resolve(value(&mut args, arg)?)?),
            "-n" | "--downloads" => {
                let n: u32 = value(&mut args, arg)?
                    .parse()
                    .map_err(|_| Error::new("--downloads needs a whole number"))?;
                options.max_downloads = (n > 0).then_some(n);
            }
            "-e" | "--expires" => {
                let text = value(&mut args, arg)?;
                options.expires_in = Some(util::parse_duration(text).ok_or_else(|| {
                    Error::new(format!(
                        "can't understand --expires {text}; try 30s, 10m, 2h or 1d"
                    ))
                })?);
            }
            "-p" | "--port" => options.port = parse_port(value(&mut args, arg)?)?,
            "--no-upnp" => options.upnp = false,
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(Error::new(format!("unknown option {flag}")))
            }
            _ if path.is_some() => {
                return Err(Error::new(
                    "share one file or folder at a time (put several in a folder)",
                ))
            }
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    options.path = path
        .ok_or_else(|| Error::new("what should be shared? Usage: beemr share <file-or-folder>"))?;
    beemr::share::run(options, profile).await.map(drop)
}

async fn get(args: &[String]) -> Result<()> {
    let profile = load_profile(true)?;
    let mut ticket = None;
    let mut output_dir = PathBuf::from(".");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => output_dir = PathBuf::from(value(&mut args, arg)?),
            _ if ticket.is_some() => return Err(Error::new("expected a single ticket")),
            _ => ticket = Some(arg.clone()),
        }
    }
    let ticket = ticket.ok_or_else(|| Error::new("missing ticket. Usage: beemr get <ticket>"))?;
    let saved = beemr::get::run(GetOptions { ticket, output_dir }, profile).await?;
    eprintln!("Saved to {}", saved.display());
    Ok(())
}

async fn msg(args: &[String]) -> Result<()> {
    let [to, words @ ..] = args else {
        return Err(Error::new("usage: beemr msg <contact-or-id> <message>"));
    };
    let profile = load_profile(true)?;
    let to = profile.contacts.resolve(to)?;
    send_message(profile, to, &words.join(" ")).await
}

async fn send_message(profile: Profile, to: DeviceId, body: &str) -> Result<()> {
    let body = message::validate_body(body)?;
    let who = profile.contacts.describe(&to, None);
    let sent_at = message::now_secs();
    let network = profile.network.clone();
    let Some(dht) = Dht::start(&network, false)? else {
        return Err(Error::new(
            "messaging needs the DHT, which is disabled in this environment",
        ));
    };
    let node = Node::start(NodeOptions {
        mode: Mode::Client,
        port: 0,
        public_network: network.public,
        upnp: network.public,
        relays: Vec::new(),
        key_seed: None,
    })
    .await?;
    eprintln!("Sending to {who}…");
    match message::send(
        &node,
        &dht,
        &profile.identity,
        &profile.name,
        to,
        &body,
        sent_at,
    )
    .await
    {
        Ok(path) => {
            eprintln!("Delivered ({}).", path.describe());
            Ok(())
        }
        Err(e) => {
            message::queue(
                &profile.config,
                &OutboxEntry {
                    to: to.to_string(),
                    body,
                    queued_at: sent_at,
                    attempts: 1,
                },
            )?;
            eprintln!("Couldn't reach {who} right now ({e}).");
            if daemon::is_running(&profile.config) {
                eprintln!("Queued: it will be delivered automatically when they're online.");
            } else {
                eprintln!(
                    "Queued. Start the background service so it's delivered automatically:\n    beemr daemon install"
                );
            }
            Ok(())
        }
    }
}

/// Inbox entries, newest first, with their display numbers.
fn numbered_inbox(config: &Config) -> Result<Vec<message::InboxEntry>> {
    let mut entries = message::inbox(config)?;
    entries.reverse();
    Ok(entries)
}

fn inbox(args: &[String]) -> Result<()> {
    let show_all = args.iter().any(|a| a == "--all" || a == "-a");
    let profile = load_profile(true)?;
    let entries = numbered_inbox(&profile.config)?;
    let unread = entries.iter().filter(|e| !e.read).count();
    if entries.is_empty() {
        eprintln!("Your inbox is empty.");
    } else {
        println!(
            "Inbox: {} message{}, {unread} new\n",
            entries.len(),
            if entries.len() == 1 { "" } else { "s" }
        );
    }
    let mut unknown = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if !show_all && entry.read && i >= 10 {
            println!("  … {} older (beemr inbox --all)", entries.len() - i);
            break;
        }
        let from = DeviceId::parse(&entry.from);
        let who = from.map_or_else(
            || entry.name.clone(),
            |id| profile.contacts.describe(&id, Some(&entry.name)),
        );
        if let Some(id) = from.filter(|id| profile.contacts.name_of(id).is_none()) {
            if !unknown.contains(&id) {
                unknown.push(id);
            }
        }
        let marker = if entry.read { " " } else { "●" };
        println!(
            "{marker} {:>2}  {who} · {}",
            i + 1,
            message::ago(entry.sent_at)
        );
        for line in entry.body.lines() {
            println!("      {line}");
        }
        println!();
    }
    if !entries.is_empty() {
        println!("Reply with: beemr reply <number> <message>");
    }
    for id in unknown.iter().take(3) {
        println!("Save a sender as a contact: beemr contact add <name> {id}");
    }
    if !daemon::is_running(&profile.config) {
        eprintln!(
            "\nNote: the background service isn't running, so new messages can't arrive.\n\
             Start it with: beemr daemon install"
        );
    }
    message::mark_all_read(&profile.config)
}

async fn reply(args: &[String]) -> Result<()> {
    let [number, words @ ..] = args else {
        return Err(Error::new("usage: beemr reply <number> <message>"));
    };
    let profile = load_profile(true)?;
    let n: usize = number
        .parse()
        .map_err(|_| Error::new("the first argument should be a message number from your inbox"))?;
    let entries = numbered_inbox(&profile.config)?;
    let entry = n
        .checked_sub(1)
        .and_then(|i| entries.get(i))
        .ok_or_else(|| Error::new(format!("there's no message number {n} in your inbox")))?;
    let to = DeviceId::parse(&entry.from)
        .ok_or_else(|| Error::new("that message has an invalid sender"))?;
    send_message(profile, to, &words.join(" ")).await
}

fn setup(args: &[String]) -> Result<()> {
    let config = Config::load()?;
    let mut name = None;
    let mut install_service = true;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = Some(value(&mut args, arg)?.to_string()),
            "--no-service" => install_service = false,
            other => return Err(Error::new(format!("unknown option {other}"))),
        }
    }
    let current = config.device_name()?.unwrap_or_else(default_device_name);
    let name = match name {
        Some(name) => name,
        None if std::io::stdin().is_terminal() => prompt_name(&current)?,
        None => current,
    };
    let name = config.set_device_name(&name)?;
    let profile = Profile::load_from(config)?;
    println!(
        "\nThis device is \"{name}\". Its ID is:\n\n    {}\n",
        profile.identity.id()
    );
    println!("Share this ID with people who want to message you or send you files.");
    if install_service {
        match service::install(&profile.config) {
            Ok(how) => println!("Background service: running, {how}."),
            Err(e) => eprintln!(
                "Couldn't start the background service automatically ({e}).\n\
                 You can still share files; to receive messages run: beemr daemon run"
            ),
        }
    }
    println!("\nTry it: beemr share <file>   ·   beemr help");
    Ok(())
}

fn show_id() -> Result<()> {
    let profile = load_profile(true)?;
    println!("{}", profile.identity.id());
    eprintln!(
        "\nThis is \"{}\". Share the ID above with people who want to message you or\n\
         send files only to this device.",
        profile.name
    );
    Ok(())
}

fn name(args: &[String]) -> Result<()> {
    let config = Config::load()?;
    if args.is_empty() {
        println!(
            "{}",
            config.device_name()?.unwrap_or_else(default_device_name)
        );
        return Ok(());
    }
    let name = config.set_device_name(&args.join(" "))?;
    eprintln!("This device is now called \"{name}\".");
    if daemon::is_running(&config) {
        eprintln!("Restart the background service to publish the new name: beemr daemon install");
    }
    Ok(())
}

fn contact(args: &[String]) -> Result<()> {
    let config = Config::load()?;
    let mut contacts = Contacts::load(&config)?;
    match args {
        [cmd, name @ .., id] if cmd == "add" && !name.is_empty() => {
            let id = DeviceId::parse(id)
                .ok_or_else(|| Error::new(format!("'{id}' is not a device ID")))?;
            let name = contacts.add(&name.join(" "), id)?;
            contacts.save()?;
            eprintln!("Saved {name}. Message them with: beemr msg \"{name}\" <message>");
        }
        [cmd, name @ ..] if (cmd == "remove" || cmd == "rm") && !name.is_empty() => {
            let name = name.join(" ");
            if !contacts.remove(&name) {
                return Err(Error::new(format!("no contact named '{name}'")));
            }
            contacts.save()?;
            eprintln!("Removed {name}.");
        }
        [cmd] if cmd == "list" || cmd == "ls" => list_contacts(&contacts),
        [] => list_contacts(&contacts),
        _ => {
            return Err(Error::new(
                "usage: beemr contact add <name> <device-id> | list | remove <name>",
            ))
        }
    }
    Ok(())
}

fn list_contacts(contacts: &Contacts) {
    if contacts.entries().is_empty() {
        eprintln!("No contacts yet. Add one with: beemr contact add <name> <device-id>");
    }
    for (name, id) in contacts.entries() {
        println!("{name}\t{id}");
    }
}

async fn daemon_command(args: &[String]) -> Result<()> {
    let (sub, rest) = args
        .split_first()
        .map_or(("status", &[][..]), |(s, r)| (s.as_str(), r));
    match sub {
        "run" => {
            let mut port = 0;
            let mut args = rest.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-p" | "--port" => port = parse_port(value(&mut args, arg)?)?,
                    other => return Err(Error::new(format!("unknown option {other}"))),
                }
            }
            daemon::run(load_profile(false)?, port).await
        }
        "install" | "start" => {
            let config = Config::load()?;
            let how = service::install(&config)?;
            eprintln!("Background service running; it {how}.");
            Ok(())
        }
        "uninstall" | "stop" => {
            let config = Config::load()?;
            service::uninstall(&config)?;
            eprintln!("Background service stopped and removed from startup.");
            Ok(())
        }
        "status" => {
            let config = Config::load()?;
            let running = daemon::is_running(&config);
            if running {
                println!("The background service is running.");
            } else {
                println!(
                    "The background service is not running. Start it with: beemr daemon install"
                );
            }
            let queued = message::outbox_len(&config)?;
            if queued > 0 {
                println!("{queued} message(s) waiting to be delivered.");
            }
            println!("Log: {}", service::log_path(&config).display());
            if !running {
                // Like `systemctl status`: a non-zero exit code means "not running",
                // so scripts can check it.
                std::process::exit(3);
            }
            Ok(())
        }
        other => Err(Error::new(format!(
            "unknown daemon command '{other}' (status, install, uninstall, run)"
        ))),
    }
}

async fn relay_command(args: &[String]) -> Result<()> {
    let (sub, rest) = match args.split_first() {
        Some((s, r)) if !s.starts_with('-') => (s.as_str(), r),
        _ => ("run", args),
    };
    match sub {
        "run" => {
            let mut port = relay::DEFAULT_PORT;
            let mut public_listing = true;
            let mut args = rest.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-p" | "--port" => port = parse_port(value(&mut args, arg)?)?,
                    "--private" => public_listing = false,
                    other => return Err(Error::new(format!("unknown option {other}"))),
                }
            }
            relay::run(load_profile(false)?, port, public_listing).await
        }
        "use" | "add" => {
            let [addr] = rest else {
                return Err(Error::new("usage: beemr relay use <address>"));
            };
            let addr: libp2p::Multiaddr = addr
                .parse()
                .map_err(|_| Error::new("that isn't a valid relay address"))?;
            if beemr::node::split_peer(&addr).is_none() {
                return Err(Error::new(
                    "the relay address must end in /p2p/<id> (copy it from `beemr relay run`)",
                ));
            }
            let config = Config::load()?;
            let mut relays = config.relays()?;
            if !relays.contains(&addr) {
                relays.push(addr);
            }
            config.set_relays(&relays)?;
            eprintln!("Saved. beemr will use this relay when a direct connection isn't possible.");
            Ok(())
        }
        "list" | "ls" => {
            let relays = Config::load()?.relays()?;
            if relays.is_empty() {
                eprintln!("No saved relays. Public beemr relays are found automatically.");
            }
            relays.iter().for_each(|r| println!("{r}"));
            Ok(())
        }
        "remove" | "rm" => {
            let [addr] = rest else {
                return Err(Error::new("usage: beemr relay remove <address>"));
            };
            let config = Config::load()?;
            let mut relays = config.relays()?;
            let before = relays.len();
            relays.retain(|r| r.to_string() != *addr);
            if relays.len() == before {
                return Err(Error::new("that relay isn't saved"));
            }
            config.set_relays(&relays)?;
            eprintln!("Removed.");
            Ok(())
        }
        other => Err(Error::new(format!(
            "unknown relay command '{other}' (run, use, list, remove)"
        ))),
    }
}

async fn dht_node() -> Result<()> {
    let network = beemr::config::NetworkSettings::from_env();
    let _dht =
        Dht::start(&network, true)?.ok_or_else(|| Error::new("couldn't start a DHT node"))?;
    eprintln!("DHT node running. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    Ok(())
}

/// The value following an option, or an error naming the option.
fn value<'a>(args: &mut impl Iterator<Item = &'a String>, option: &str) -> Result<&'a str> {
    args.next()
        .map(String::as_str)
        .ok_or_else(|| Error::new(format!("{option} needs a value")))
}

fn parse_port(text: &str) -> Result<u16> {
    text.parse()
        .map_err(|_| Error::new("--port needs a number from 0 to 65535"))
}
