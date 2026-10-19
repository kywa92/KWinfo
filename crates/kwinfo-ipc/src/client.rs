//! The client: [`HelperBackend`], a [`Backend`] that merges the helper's devices and
//! readings into the sampler, and [`HelperHandle`], which the UI uses to see how the
//! helper is doing and to start a session helper.
//!
//! A worker thread owns the connection. It first tries the D-Bus system service (and
//! tries again every [`RETRY`] while it is missing, so a fresh install is picked up),
//! or, when asked, starts a session helper (`pkexec kwinfo-helper stdio`), which asks
//! for an administrator password once and lives until KWinfo quits.

use std::io::{self, BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use kwinfo_core::{Backend, Channel, Device, Sample};
use tracing::{debug, info, warn};

use crate::framing::{read_message, write_message};
use crate::{
    Capabilities, DeviceEntry, LOCAL_HELPER_PATH, LOCAL_MANIFEST_PATH, PACKAGED_HELPER_PATH,
    PROTOCOL_VERSION, Request, Response,
};

/// How often a missing service is looked for again.
pub const RETRY: Duration = Duration::from_secs(30);
/// How long the sampler's first enumeration waits for the first connection attempt, so
/// that the helper's rows are there from the first tick.
pub const FIRST_CONTACT: Duration = Duration::from_millis(1500);
/// How long a sampling tick waits for the helper's readings before using the last ones.
pub const SAMPLE_WAIT: Duration = Duration::from_millis(250);
/// How often the device list is fetched again (drives that woke up, for instance).
const REFRESH_DEVICES: Duration = Duration::from_secs(60);
/// A session helper's first answer waits for the user to authenticate.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// How the helper is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The D-Bus system service.
    Service,
    /// A helper started through `pkexec` for this session.
    Session,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HelperState {
    /// Not used: not Linux, or the data is not live (mock, replay). The text says why.
    Unavailable(String),
    Connecting,
    Running {
        via: Via,
        version: String,
        capabilities: Capabilities,
    },
    /// No system service is installed.
    NotInstalled,
    /// A session helper is waiting for the user to authenticate.
    Authenticating,
    /// polkit (or the user) said no.
    Denied(String),
    Failed(String),
}

impl HelperState {
    pub fn is_running(&self) -> bool {
        matches!(self, HelperState::Running { .. })
    }

    /// One sentence for tooltips and logs.
    pub fn describe(&self) -> String {
        match self {
            HelperState::Unavailable(why) => why.clone(),
            HelperState::Connecting => "Connecting to the helper…".to_string(),
            HelperState::Running {
                via,
                version,
                capabilities,
            } => format!(
                "The helper {version} is running ({}); it reads {}.",
                match via {
                    Via::Service => "system service",
                    Via::Session => "for this session",
                },
                capabilities.summary()
            ),
            HelperState::NotInstalled => {
                "The helper is not installed, so root-only sensors (CPU package power, \
                 S.M.A.R.T., GPU load) are missing."
                    .to_string()
            }
            HelperState::Authenticating => "Waiting for an administrator password…".to_string(),
            HelperState::Denied(why) => format!("The helper was refused: {why}"),
            HelperState::Failed(why) => format!("The helper failed: {why}"),
        }
    }
}

/// How to start a session helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl SessionCommand {
    /// `pkexec <helper> stdio`: polkit asks for an administrator password.
    pub fn pkexec(helper: &Path) -> Self {
        Self {
            program: PathBuf::from("pkexec"),
            args: vec![helper.display().to_string(), "stdio".to_string()],
        }
    }

    /// Any command that speaks the stdio protocol, e.g. the helper without `pkexec`.
    pub fn direct(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    /// Use the D-Bus system service, looking for it again every [`RETRY`].
    pub service: bool,
    /// Start a session helper right away.
    pub session: Option<SessionCommand>,
}

#[derive(Debug)]
enum LinkError {
    NotInstalled,
    Denied(String),
    Failed(String),
}

impl LinkError {
    fn state(self) -> HelperState {
        match self {
            LinkError::NotInstalled => HelperState::NotInstalled,
            LinkError::Denied(why) => HelperState::Denied(why),
            LinkError::Failed(why) => HelperState::Failed(why),
        }
    }
}

/// A connection to a helper.
trait Link: Send {
    fn via(&self) -> Via;
    fn call(&mut self, request: &Request) -> Result<Response, LinkError>;
}

#[cfg(unix)]
mod service {
    //! The D-Bus system service as a [`Link`].

    use super::{Link, LinkError, Request, Response, Via};
    use crate::{DBUS_INTERFACE, DBUS_NAME, DBUS_PATH};
    use std::time::Duration;

    pub(super) struct ServiceLink {
        proxy: zbus::blocking::Proxy<'static>,
    }

    /// What a D-Bus error means for the user.
    pub(super) fn classify(err: zbus::Error) -> LinkError {
        let named = |name: &str, detail: String| match name {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => LinkError::NotInstalled,
            "org.freedesktop.DBus.Error.AccessDenied" => LinkError::Denied(detail),
            _ => LinkError::Failed(format!("{name}: {detail}")),
        };
        match err {
            zbus::Error::MethodError(name, detail, _) => {
                named(name.as_str(), detail.unwrap_or_default())
            }
            zbus::Error::FDO(err) => match *err {
                zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                    LinkError::NotInstalled
                }
                zbus::fdo::Error::AccessDenied(detail) => LinkError::Denied(detail),
                other => LinkError::Failed(other.to_string()),
            },
            other => LinkError::Failed(other.to_string()),
        }
    }

    impl ServiceLink {
        pub(super) fn connect() -> Result<Self, LinkError> {
            let connection = zbus::blocking::connection::Builder::system()
                .and_then(|b| b.method_timeout(Duration::from_secs(10)).build())
                .map_err(|err| LinkError::Failed(format!("no system bus: {err}")))?;
            let proxy =
                zbus::blocking::Proxy::new_owned(connection, DBUS_NAME, DBUS_PATH, DBUS_INTERFACE)
                    .map_err(classify)?;
            Ok(Self { proxy })
        }
    }

    impl Link for ServiceLink {
        fn via(&self) -> Via {
            Via::Service
        }

        fn call(&mut self, request: &Request) -> Result<Response, LinkError> {
            let json: String = match request {
                Request::Hello { protocol } => self.proxy.call("Hello", &(*protocol,)),
                Request::Capabilities => self.proxy.call("Capabilities", &()),
                Request::Enumerate => self.proxy.call("Enumerate", &()),
                Request::Sample => self.proxy.call("Sample", &()),
            }
            .map_err(classify)?;
            serde_json::from_str(&json)
                .map_err(|err| LinkError::Failed(format!("bad answer: {err}")))
        }
    }
}

/// A session helper: a child process speaking the stdio protocol.
struct SessionLink {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<io::Result<Response>>,
    answered: bool,
}

/// What a session helper's exit status means: 126 and 127 are `pkexec`'s own.
fn exit_meaning(code: Option<i32>) -> LinkError {
    match code {
        Some(126) => LinkError::Denied("the administrator password was not given".to_string()),
        Some(127) => LinkError::Denied(
            "polkit did not allow it (is a polkit authentication agent running?)".to_string(),
        ),
        Some(code) => LinkError::Failed(format!("the session helper exited with status {code}")),
        None => LinkError::Failed("the session helper was killed".to_string()),
    }
}

impl SessionLink {
    fn spawn(command: &SessionCommand) -> Result<Self, LinkError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                LinkError::Failed(format!(
                    "{} could not be started: {err}",
                    command.program.display()
                ))
            })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(LinkError::Failed(
                "the session helper has no pipes".to_string(),
            ));
        };
        let (tx, answers) = mpsc::channel();
        thread::Builder::new()
            .name("kwinfo-helper-out".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let (answer, more) = match read_message::<_, Response>(&mut reader) {
                        Ok(Some(response)) => (Ok(response), true),
                        Ok(None) => break,
                        // A line that is not a message; the next one may be fine.
                        Err(err) if err.kind() == io::ErrorKind::InvalidData => (Err(err), true),
                        Err(err) => (Err(err), false),
                    };
                    if tx.send(answer).is_err() || !more {
                        break;
                    }
                }
            })
            .map_err(|err| LinkError::Failed(err.to_string()))?;
        // Whatever the helper logs goes into KWinfo's log.
        let _ = thread::Builder::new()
            .name("kwinfo-helper-err".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    info!(target: "kwinfo::helper", "{line}");
                }
            });
        Ok(Self {
            child,
            stdin,
            answers,
            answered: false,
        })
    }

    /// Why the helper went away.
    fn gone(&mut self) -> LinkError {
        // The pipes closed; the process is about to be reaped.
        for _ in 0..50 {
            if let Ok(Some(status)) = self.child.try_wait() {
                return exit_meaning(status.code());
            }
            thread::sleep(Duration::from_millis(20));
        }
        LinkError::Failed("the session helper stopped answering".to_string())
    }
}

impl Link for SessionLink {
    fn via(&self) -> Via {
        Via::Session
    }

    fn call(&mut self, request: &Request) -> Result<Response, LinkError> {
        if write_message(&mut self.stdin, request).is_err() {
            return Err(self.gone());
        }
        let timeout = if self.answered {
            CALL_TIMEOUT
        } else {
            AUTH_TIMEOUT
        };
        match self.answers.recv_timeout(timeout) {
            Ok(Ok(response)) => {
                self.answered = true;
                Ok(response)
            }
            Ok(Err(err)) => Err(LinkError::Failed(format!("bad answer: {err}"))),
            Err(RecvTimeoutError::Timeout) => Err(LinkError::Failed(
                "the session helper stopped answering".to_string(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(self.gone()),
        }
    }
}

impl Drop for SessionLink {
    fn drop(&mut self) {
        // Closing standard input ends the helper; pkexec passes that on.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Data {
    devices: Vec<DeviceEntry>,
    samples: Vec<Sample>,
    /// Counts the answers to `Sample`, so that a tick can wait for a fresh one.
    sample_seq: u64,
    /// The devices changed since the sampler last enumerated.
    devices_changed: bool,
    /// The first connection attempt finished, one way or the other.
    settled: bool,
}

struct Shared {
    state: Mutex<HelperState>,
    data: Mutex<Data>,
    changed: Condvar,
}

impl Shared {
    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn state(&self) -> HelperState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_state(&self, state: HelperState) {
        let mut current = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *current != state {
            info!(target: "kwinfo::helper", "{}", state.describe());
            *current = state;
        }
        drop(current);
        self.changed.notify_all();
    }
}

/// What the backend and the handle ask of the worker.
enum Order {
    Sample,
    Retry,
    StartSession(SessionCommand),
    StopSession,
    Stop,
}

/// The UI's view of the helper. Cheap to clone.
#[derive(Clone)]
pub struct HelperHandle {
    shared: Arc<Shared>,
    tx: Sender<Order>,
}

impl HelperHandle {
    pub fn state(&self) -> HelperState {
        self.shared.state()
    }

    /// Looks for the system service again now (after installing it, for instance).
    pub fn retry(&self) {
        let _ = self.tx.send(Order::Retry);
    }

    /// Starts a session helper, replacing the current connection.
    pub fn start_session(&self, command: SessionCommand) {
        let _ = self.tx.send(Order::StartSession(command));
    }

    /// Ends a session helper and goes back to the system service.
    pub fn stop_session(&self) {
        let _ = self.tx.send(Order::StopSession);
    }

    /// Waits until the first connection attempt has finished, or `timeout`.
    pub fn wait_settled(&self, timeout: Duration) -> HelperState {
        let deadline = Instant::now() + timeout;
        let mut data = self.shared.data();
        while !data.settled {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            data = self
                .shared
                .changed
                .wait_timeout(data, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(data);
        self.state()
    }
}

/// The helper as a [`Backend`] of the sampler. See the module documentation.
pub struct HelperBackend {
    shared: Arc<Shared>,
    tx: Sender<Order>,
    waited: bool,
}

impl Backend for HelperBackend {
    fn name(&self) -> &'static str {
        "helper"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        if !std::mem::replace(&mut self.waited, true) {
            let deadline = Instant::now() + FIRST_CONTACT;
            let mut data = self.shared.data();
            while !data.settled && Instant::now() < deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                data = self
                    .shared
                    .changed
                    .wait_timeout(data, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        let mut data = self.shared.data();
        data.devices_changed = false;
        Ok(data
            .devices
            .iter()
            .map(|entry| (entry.device.clone(), entry.channels.clone()))
            .collect())
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        if !self.shared.state().is_running() {
            return Ok(());
        }
        let deadline = Instant::now() + SAMPLE_WAIT;
        let mut data = self.shared.data();
        let seq = data.sample_seq;
        if self.tx.send(Order::Sample).is_err() {
            return Ok(());
        }
        while data.sample_seq == seq {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                debug!("the helper's readings are late; using the previous ones");
                break;
            }
            data = self
                .shared
                .changed
                .wait_timeout(data, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        out.extend(data.samples.iter().cloned());
        Ok(())
    }

    /// Only when the helper's devices changed: right after connecting, for instance.
    fn rescan_every(&self) -> Option<Duration> {
        if self.shared.data().devices_changed {
            Some(Duration::ZERO)
        } else {
            Some(Duration::from_secs(3600))
        }
    }
}

impl Drop for HelperBackend {
    fn drop(&mut self) {
        let _ = self.tx.send(Order::Stop);
    }
}

/// Starts the worker; see the module documentation.
pub fn start(options: ClientOptions) -> (HelperBackend, HelperHandle) {
    let shared = Arc::new(Shared {
        state: Mutex::new(HelperState::Connecting),
        data: Mutex::new(Data::default()),
        changed: Condvar::new(),
    });
    let (tx, rx) = mpsc::channel();
    let worker = Arc::clone(&shared);
    let spawned = thread::Builder::new()
        .name("kwinfo-helper-client".into())
        .spawn(move || Worker::new(worker, options).run(rx));
    if let Err(err) = spawned {
        shared.set_state(HelperState::Failed(format!("no worker thread: {err}")));
        shared.data().settled = true;
    }
    (
        HelperBackend {
            shared: Arc::clone(&shared),
            tx: tx.clone(),
            waited: false,
        },
        HelperHandle { shared, tx },
    )
}

/// A handle for when there is no helper at all, saying why.
pub fn unavailable(why: impl Into<String>) -> HelperHandle {
    fixed(HelperState::Unavailable(why.into()))
}

/// A handle that always reports `state`: for tests and previews of the UI.
pub fn fixed(state: HelperState) -> HelperHandle {
    let shared = Arc::new(Shared {
        state: Mutex::new(state),
        data: Mutex::new(Data {
            settled: true,
            ..Data::default()
        }),
        changed: Condvar::new(),
    });
    let (tx, _) = mpsc::channel();
    HelperHandle { shared, tx }
}

struct Worker {
    shared: Arc<Shared>,
    service: bool,
    session: Option<SessionCommand>,
    link: Option<Box<dyn Link>>,
    retry_at: Option<Instant>,
    devices_at: Option<Instant>,
}

impl Worker {
    fn new(shared: Arc<Shared>, options: ClientOptions) -> Self {
        Self {
            shared,
            service: options.service,
            retry_at: options.service.then(Instant::now),
            session: options.session,
            link: None,
            devices_at: None,
        }
    }

    fn run(mut self, rx: Receiver<Order>) {
        loop {
            self.connect_if_due();
            let wait = self.retry_at.map_or(Duration::from_secs(3600), |t| {
                t.saturating_duration_since(Instant::now())
            });
            let order = match rx.recv_timeout(wait.max(Duration::from_millis(10))) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            match order {
                Order::Sample => {
                    // Ticks that queued up while connecting need one answer, not many.
                    while let Ok(Order::Sample) = rx.try_recv() {}
                    self.sample();
                }
                Order::Retry => {
                    if self.link.is_none() {
                        self.retry_at = Some(Instant::now());
                    }
                }
                Order::StartSession(command) => {
                    self.link = None;
                    self.session = Some(command);
                }
                Order::StopSession => {
                    if self.link.as_ref().is_some_and(|l| l.via() == Via::Session) {
                        self.link = None;
                        self.shared.set_state(HelperState::Connecting);
                        self.retry_at = self.service.then(Instant::now);
                    }
                }
                Order::Stop => return,
            }
        }
    }

    fn connect_if_due(&mut self) {
        if self.link.is_some() {
            return;
        }
        let attempt = if let Some(command) = self.session.take() {
            self.shared.set_state(HelperState::Authenticating);
            Some(SessionLink::spawn(&command).map(|l| Box::new(l) as Box<dyn Link>))
        } else if self.service && self.retry_at.is_some_and(|t| Instant::now() >= t) {
            Some(connect_service())
        } else {
            None
        };
        let Some(attempt) = attempt else {
            return;
        };
        let result = attempt.and_then(|mut link| self.handshake(link.as_mut()).map(|()| link));
        match result {
            Ok(link) => {
                self.link = Some(link);
                self.retry_at = None;
            }
            Err(err) => {
                self.shared.set_state(err.state());
                self.retry_at = self.service.then(|| Instant::now() + RETRY);
            }
        }
        self.shared.data().settled = true;
        self.shared.changed.notify_all();
    }

    /// Hello, Capabilities, Enumerate; then the helper counts as running.
    fn handshake(&mut self, link: &mut dyn Link) -> Result<(), LinkError> {
        let version = match link.call(&Request::Hello {
            protocol: PROTOCOL_VERSION,
        })? {
            Response::Hello { version, .. } => version,
            Response::Error { message } => return Err(LinkError::Failed(message)),
            other => return Err(LinkError::Failed(format!("unexpected answer {other:?}"))),
        };
        let capabilities = match link.call(&Request::Capabilities)? {
            Response::Capabilities(caps) => caps,
            other => return Err(LinkError::Failed(format!("unexpected answer {other:?}"))),
        };
        self.fetch_devices(link)?;
        for note in &capabilities.notes {
            info!(target: "kwinfo::helper", "{note}");
        }
        self.shared.set_state(HelperState::Running {
            via: link.via(),
            version,
            capabilities,
        });
        Ok(())
    }

    fn fetch_devices(&mut self, link: &mut dyn Link) -> Result<(), LinkError> {
        let Response::Enumerate { devices } = link.call(&Request::Enumerate)? else {
            return Err(LinkError::Failed(
                "unexpected answer to Enumerate".to_string(),
            ));
        };
        let mut data = self.shared.data();
        if data.devices != devices {
            data.devices = devices;
            data.devices_changed = true;
        }
        drop(data);
        self.devices_at = Some(Instant::now());
        Ok(())
    }

    fn sample(&mut self) {
        let Some(mut link) = self.link.take() else {
            return;
        };
        let refresh = self
            .devices_at
            .is_none_or(|t| t.elapsed() >= REFRESH_DEVICES);
        let result = (if refresh {
            self.fetch_devices(link.as_mut())
        } else {
            Ok(())
        })
        .and_then(|()| link.call(&Request::Sample));
        match result {
            Ok(Response::Samples { samples }) => {
                let mut data = self.shared.data();
                data.samples = samples
                    .into_iter()
                    .map(|s| Sample::new(s.channel, s.value))
                    .collect();
                data.sample_seq += 1;
                drop(data);
                self.link = Some(link);
            }
            Ok(other) => {
                warn!(target: "kwinfo::helper", "unexpected answer to Sample: {other:?}");
                self.link = Some(link);
            }
            Err(err) => {
                warn!(target: "kwinfo::helper", "lost the helper: {err:?}");
                self.shared.set_state(err.state());
                // A lost service is looked for again soon; a lost session is not
                // restarted, which would ask for the password again.
                self.retry_at = self
                    .service
                    .then(|| Instant::now() + Duration::from_secs(2));
            }
        }
        self.shared.changed.notify_all();
    }
}

#[cfg(unix)]
fn connect_service() -> Result<Box<dyn Link>, LinkError> {
    service::ServiceLink::connect().map(|l| Box::new(l) as Box<dyn Link>)
}

#[cfg(not(unix))]
fn connect_service() -> Result<Box<dyn Link>, LinkError> {
    Err(LinkError::Failed(
        "the helper only runs on Linux".to_string(),
    ))
}

/// The `kwinfo-helper` that belongs to this KWinfo: next to the running executable (a
/// build, or the AppImage's `usr/bin`), else a packaged or locally installed one.
pub fn find_helper() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("kwinfo-helper")));
    beside
        .into_iter()
        .chain([PACKAGED_HELPER_PATH, LOCAL_HELPER_PATH].map(PathBuf::from))
        .find(|p| p.is_file())
}

/// `helper`, or a copy root can run: an AppImage's files are in a FUSE mount that only
/// its user can read, so `pkexec` would fail on them. The copy goes to
/// `$XDG_RUNTIME_DIR/kwinfo/`.
pub fn runnable_by_root(helper: &Path) -> io::Result<PathBuf> {
    let mounted = std::env::var_os("APPDIR").is_some_and(|dir| helper.starts_with(dir));
    if std::env::var_os("APPIMAGE").is_none() && !mounted {
        return Ok(helper.to_path_buf());
    }
    use std::os::unix::fs::PermissionsExt as _;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    let dir = PathBuf::from(runtime).join("kwinfo");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let copy = dir.join("kwinfo-helper");
    std::fs::copy(helper, &copy)?;
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755))?;
    Ok(copy)
}

/// Whether `kwinfo-helper install` put the helper in place on this machine.
pub fn installed_locally() -> bool {
    Path::new(LOCAL_MANIFEST_PATH).exists()
}

/// Whether a package installed the helper.
pub fn installed_by_package() -> bool {
    Path::new(PACKAGED_HELPER_PATH).exists()
}

/// Runs `pkexec <helper> <args>` (install, uninstall) and returns what it printed, or
/// what went wrong, for the user. Blocks until the user has authenticated.
pub fn pkexec(helper: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("pkexec")
        .arg(helper)
        .args(args)
        .output()
        .map_err(|err| format!("pkexec could not be started ({err}); is polkit installed?"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(match exit_meaning(output.status.code()) {
        LinkError::Denied(why) => format!("Not done: {why}."),
        _ if !stderr.is_empty() => stderr,
        _ => format!(
            "{} {} failed ({})",
            helper.display(),
            args.join(" "),
            output.status
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkexec_exit_statuses_are_explained() {
        assert!(
            matches!(exit_meaning(Some(126)), LinkError::Denied(why) if why.contains("password"))
        );
        assert!(matches!(exit_meaning(Some(127)), LinkError::Denied(why) if why.contains("agent")));
        assert!(matches!(exit_meaning(Some(1)), LinkError::Failed(_)));
    }

    #[cfg(unix)]
    #[test]
    fn dbus_errors_become_states() {
        use zbus::fdo;
        let state = |err: zbus::Error| service::classify(err).state();
        assert_eq!(
            state(zbus::Error::FDO(Box::new(fdo::Error::ServiceUnknown(
                "x".into()
            )))),
            HelperState::NotInstalled
        );
        assert_eq!(
            state(zbus::Error::FDO(Box::new(fdo::Error::AccessDenied(
                "inactive".into()
            )))),
            HelperState::Denied("inactive".into())
        );
        assert!(matches!(
            state(zbus::Error::InvalidReply),
            HelperState::Failed(_)
        ));
    }

    #[test]
    fn a_cancelled_password_prompt_is_a_refusal() {
        let (_backend, handle) = start(ClientOptions {
            service: false,
            session: Some(SessionCommand::direct("/bin/sh", ["-c", "exit 126"])),
        });
        let state = handle.wait_settled(Duration::from_secs(10));
        assert!(
            matches!(&state, HelperState::Denied(why) if why.contains("password")),
            "{state:?}"
        );
        assert!(state.describe().starts_with("The helper was refused"));
    }

    #[test]
    fn a_missing_program_is_a_failure() {
        let (mut backend, handle) = start(ClientOptions {
            service: false,
            session: Some(SessionCommand::direct(
                "/nonexistent/kwinfo-helper",
                ["stdio"],
            )),
        });
        assert!(matches!(
            handle.wait_settled(Duration::from_secs(10)),
            HelperState::Failed(_)
        ));
        assert!(backend.enumerate().unwrap().is_empty());
        let mut out = Vec::new();
        backend.sample(&mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn states_describe_themselves() {
        let running = HelperState::Running {
            via: Via::Service,
            version: "0.1.0".into(),
            capabilities: Capabilities {
                rapl: true,
                ..Capabilities::default()
            },
        };
        assert!(running.is_running());
        assert_eq!(
            running.describe(),
            "The helper 0.1.0 is running (system service); it reads RAPL power."
        );
        assert!(!HelperState::NotInstalled.is_running());
        assert_eq!(
            unavailable("Mock data").state(),
            HelperState::Unavailable("Mock data".into())
        );
    }
}
