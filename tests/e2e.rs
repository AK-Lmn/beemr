//! End-to-end tests: run the real binary as separate devices, each with its
//! own identity, over real sockets. Public networks are disabled; tests that
//! need discovery run a private Mainline DHT in this process.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// A private DHT shared by the tests that need device discovery.
fn testnet() -> &'static Vec<String> {
    static TESTNET: OnceLock<(mainline::Testnet, Vec<String>)> = OnceLock::new();
    &TESTNET
        .get_or_init(|| {
            let testnet = mainline::Testnet::builder(10).build().unwrap();
            let bootstrap = testnet.bootstrap.clone();
            (testnet, bootstrap)
        })
        .1
}

/// A simulated device: its own config folder (identity, name, contacts,
/// inbox) and a folder downloads are saved into.
struct Device {
    root: PathBuf,
    with_dht: bool,
}

impl Device {
    fn new(name: &str) -> Device {
        Self::create(name, false)
    }

    /// A device that can find others through the test DHT.
    fn online(name: &str) -> Device {
        Self::create(name, true)
    }

    fn create(name: &str, with_dht: bool) -> Device {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "beemr-e2e-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            name.replace(' ', "-")
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("downloads")).unwrap();
        let device = Device { root, with_dht };
        assert_ok(&device.beemr().args(["name", name]).output().unwrap());
        device
    }

    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    fn beemr(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_beemr"));
        cmd.env("BEEMR_HOME", self.root.join("config"))
            .env("BEEMR_ISOLATED", "1")
            .stdin(Stdio::null());
        if self.with_dht {
            cmd.env("BEEMR_DHT_BOOTSTRAP", testnet().join(","))
                .env("BEEMR_DHT_BIND", "127.0.0.1");
        }
        cmd
    }

    fn id(&self) -> String {
        let out = self.beemr().arg("id").output().unwrap();
        assert_ok(&out);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Start sharing and return the running process plus its ticket.
    fn share(&self, path: &Path, extra: &[&str]) -> (Running, String) {
        let mut child = self
            .beemr()
            .arg("share")
            .arg(path)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let ticket = stdout
            .lines()
            .map(Result::unwrap)
            .find_map(|line| line.trim().strip_prefix("beemr get ").map(str::to_string))
            .expect("share should print a `beemr get` command");
        (Running(Some(child)), ticket)
    }

    fn get(&self, ticket: &str) -> Output {
        self.beemr()
            .args(["get", ticket, "-o"])
            .arg(self.downloads())
            .output()
            .unwrap()
    }

    /// Run the background service.
    fn daemon(&self) -> Running {
        let child = self
            .beemr()
            .args(["daemon", "run"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Running(Some(child))
    }

    fn inbox(&self) -> String {
        let out = self.beemr().arg("inbox").output().unwrap();
        assert_ok(&out);
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A running beemr process, killed if the test ends early.
struct Running(Option<Child>);

impl Running {
    fn wait(mut self) -> Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_ok(out: &Output) {
    assert!(out.status.success(), "command failed:\n{}", stderr(out));
}

/// Retry `check` until it passes or the timeout expires.
fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Compare two directory trees by path and content.
fn assert_same_tree(a: &Path, b: &Path) {
    let names = |dir: &Path| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(a), names(b), "{} vs {}", a.display(), b.display());
    for name in names(a) {
        let (pa, pb) = (a.join(&name), b.join(&name));
        if pa.is_dir() {
            assert_same_tree(&pa, &pb);
        } else {
            assert_eq!(
                fs::read(&pa).unwrap(),
                fs::read(&pb).unwrap(),
                "{}",
                pa.display()
            );
        }
    }
}

#[test]
fn sends_a_file_once_by_default() {
    let (alice, bob) = (Device::new("Alice's laptop"), Device::new("Bob"));
    let file = alice.root.join("report.bin");
    let data: Vec<u8> = (0..1_000_003u32).map(|i| (i * 31 % 251) as u8).collect();
    fs::write(&file, &data).unwrap();

    let (sharer, ticket) = alice.share(&file, &[]);
    let out = bob.get(&ticket);
    assert_ok(&out);
    assert!(stderr(&out).contains("Alice's laptop"), "{}", stderr(&out));
    assert_eq!(fs::read(bob.downloads().join("report.bin")).unwrap(), data);

    // The default limit is one download, so the sharer stops by itself.
    let out = sharer.wait();
    assert_ok(&out);
    assert!(stderr(&out).contains("download limit reached"));
    assert!(!bob.get(&ticket).status.success());
}

#[test]
fn sends_a_folder_tree() {
    let (alice, bob) = (Device::new("Alice"), Device::new("Bob"));
    let folder = alice.root.join("project");
    fs::create_dir_all(folder.join("src/nested/deeper")).unwrap();
    fs::create_dir_all(folder.join("empty-dir")).unwrap();
    fs::write(folder.join("README.md"), "hello").unwrap();
    fs::write(folder.join("empty.txt"), "").unwrap();
    fs::write(
        folder.join("src/nested/deeper/data.bin"),
        vec![7u8; 300_000],
    )
    .unwrap();
    fs::write(folder.join("src/ünïcode name.txt"), "ok").unwrap();

    let (sharer, ticket) = alice.share(&folder, &[]);
    assert_ok(&bob.get(&ticket));
    assert_ok(&sharer.wait());
    assert_same_tree(&folder, &bob.downloads().join("project"));
    // No staging folders are left behind.
    assert_eq!(fs::read_dir(bob.downloads()).unwrap().count(), 1);
}

#[test]
fn only_the_chosen_device_can_download() {
    let (alice, bob, eve) = (Device::new("Alice"), Device::new("Bob"), Device::new("Eve"));
    let file = alice.root.join("secret.txt");
    fs::write(&file, "for bob only").unwrap();

    let (sharer, ticket) = alice.share(&file, &["--to", &bob.id()]);

    let denied = eve.get(&ticket);
    assert!(!denied.status.success());
    assert!(
        stderr(&denied).contains("different device"),
        "{}",
        stderr(&denied)
    );
    assert_eq!(fs::read_dir(eve.downloads()).unwrap().count(), 0);

    assert_ok(&bob.get(&ticket));
    assert_eq!(
        fs::read_to_string(bob.downloads().join("secret.txt")).unwrap(),
        "for bob only"
    );
    assert_ok(&sharer.wait());
}

#[test]
fn contacts_can_be_used_with_to() {
    let (alice, bob) = (Device::new("Alice"), Device::new("Bob"));
    assert_ok(
        &alice
            .beemr()
            .args(["contact", "add", "Bob", "Smith", &bob.id()])
            .output()
            .unwrap(),
    );
    let file = alice.root.join("note.txt");
    fs::write(&file, "hi bob").unwrap();

    let (sharer, ticket) = alice.share(&file, &["--to", "bob smith"]);
    assert_ok(&bob.get(&ticket));
    let out = sharer.wait();
    assert_ok(&out);
    // The sender sees the contact name, not the ID.
    assert!(
        stderr(&out).contains("Sending to Bob Smith"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn download_limit_allows_exactly_n() {
    let (alice, bob) = (Device::new("Alice"), Device::new("Bob"));
    let file = alice.root.join("a.txt");
    fs::write(&file, "twice").unwrap();

    let (sharer, ticket) = alice.share(&file, &["--downloads", "2"]);
    assert_ok(&bob.get(&ticket));
    assert_ok(&bob.get(&ticket));
    // Name collisions get a numbered copy instead of overwriting.
    assert_eq!(
        fs::read_to_string(bob.downloads().join("a (1).txt")).unwrap(),
        "twice"
    );
    assert_ok(&sharer.wait());
    assert!(!bob.get(&ticket).status.success());
}

#[test]
fn expired_share_refuses_downloads() {
    let (alice, bob) = (Device::new("Alice"), Device::new("Bob"));
    let file = alice.root.join("late.txt");
    fs::write(&file, "too late").unwrap();

    let (sharer, ticket) = alice.share(&file, &["--expires", "1s"]);
    thread::sleep(Duration::from_millis(1500));
    let out = sharer.wait();
    assert_ok(&out);
    assert!(stderr(&out).contains("expired"), "{}", stderr(&out));
    assert!(!bob.get(&ticket).status.success());
}

#[test]
fn rejects_an_invalid_ticket() {
    let bob = Device::new("Bob");
    let out = bob.get("definitely-not-a-ticket");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("ticket"));
}

#[test]
fn identity_and_name_are_stable() {
    let alice = Device::new("Alice's laptop");
    let id = alice.id();
    assert_eq!(id.len(), 52);
    assert_eq!(alice.id(), id);
    let out = alice.beemr().arg("name").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "Alice's laptop"
    );
}

#[test]
fn finds_the_sharer_through_the_dht() {
    let (alice, bob) = (Device::online("Alice"), Device::online("Bob"));
    let file = alice.root.join("via-dht.txt");
    fs::write(&file, "found you").unwrap();
    let (sharer, ticket) = alice.share(&file, &[]);

    // Remove the LAN hints so the only way to find Alice is her DHT record.
    let mut parsed = beemr::ticket::Ticket::decode(&ticket).unwrap();
    parsed.lan.clear();
    let out = bob.get(&parsed.encode());
    assert_ok(&out);
    assert_eq!(
        fs::read_to_string(bob.downloads().join("via-dht.txt")).unwrap(),
        "found you"
    );
    assert_ok(&sharer.wait());
}

#[test]
fn messages_arrive_in_the_inbox_with_the_sender_name() {
    let (alice, bob) = (
        Device::online("Alice's laptop"),
        Device::online("Bob's desktop"),
    );
    let _bob_service = bob.daemon();
    let bob_id = bob.id();

    // Bob's service needs a moment to publish where it can be reached.
    let delivered = eventually(Duration::from_secs(180), || {
        let out = alice
            .beemr()
            .args(["msg", &bob_id, "hello", "from", "alice"])
            .output()
            .unwrap();
        stderr(&out).contains("Delivered")
    });
    assert!(delivered, "message was never delivered");

    let inbox = bob.inbox();
    assert!(inbox.contains("hello from alice"), "{inbox}");
    assert!(inbox.contains("Alice's laptop"), "{inbox}");
    assert!(inbox.contains("not in contacts"), "{inbox}");

    // Once saved as a contact, Bob sees his own name for Alice.
    assert_ok(
        &bob.beemr()
            .args(["contact", "add", "Alice", &alice.id()])
            .output()
            .unwrap(),
    );
    let inbox = bob.inbox();
    assert!(inbox.contains("Alice ·"), "{inbox}");
    assert!(!inbox.contains("not in contacts"), "{inbox}");
}

#[test]
fn queued_messages_are_delivered_when_the_recipient_comes_online() {
    let (alice, bob) = (Device::online("Alice"), Device::online("Bob"));
    let bob_id = bob.id();

    // Bob is offline: the message is queued.
    let out = alice
        .beemr()
        .args(["msg", &bob_id, "are you there?"])
        .output()
        .unwrap();
    assert_ok(&out);
    assert!(stderr(&out).contains("Queued"), "{}", stderr(&out));

    // Both background services start; Alice's retries the queue.
    let _bob_service = bob.daemon();
    let _alice_service = alice.daemon();
    let arrived = eventually(Duration::from_secs(150), || {
        bob.inbox().contains("are you there?")
    });
    assert!(arrived, "queued message never arrived");
}
