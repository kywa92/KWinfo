//! The D-Bus system service, `io.github.kywa92.KWinfo.Helper` (PLAN.md 3.5).
//!
//! D-Bus activation starts `kwinfo-helper serve` as root on the first call (the files
//! `kwinfo-helper install` or a package put in place say how). Every method answers with
//! the JSON of the corresponding [`Response`]; callers are authorised once per
//! connection through polkit (action `io.github.kywa92.KWinfo.read-sensors`: allowed for
//! active local sessions, refused for remote and inactive ones). The service exits after
//! [`IDLE_EXIT`] without calls; the next call starts it again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kwinfo_ipc::{DBUS_NAME, DBUS_PATH, POLKIT_ACTION, Request, Response};
use tracing::{info, warn};
use zbus::message::Header;
use zbus::zvariant::Value;
use zbus::{Connection, fdo, interface, proxy};

use crate::server::Helper;

/// The service exits when nobody has called for this long.
pub const IDLE_EXIT: Duration = Duration::from_secs(120);

#[proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority",
    gen_blocking = false
)]
trait Authority {
    /// `CheckAuthorization(subject (sa{sv}), action_id, details, flags, cancellation_id)`
    /// → `(is_authorized, is_challenge, details)`.
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, Value<'_>>),
        action_id: &str,
        details: &HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// Who may call.
#[derive(Clone)]
pub enum Authorizer {
    /// Ask polkit, remembering the bus names it allowed (a unique name is never reused).
    Polkit(Arc<Mutex<HashSet<String>>>),
    /// Everyone: for peer-to-peer connections in tests.
    AllowAll,
}

impl Authorizer {
    pub fn polkit() -> Self {
        Self::Polkit(Arc::default())
    }

    async fn check(&self, sender: Option<&str>, connection: &Connection) -> fdo::Result<()> {
        let Self::Polkit(granted) = self else {
            return Ok(());
        };
        let Some(sender) = sender else {
            return Err(fdo::Error::AccessDenied("the call has no sender".into()));
        };
        if granted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(sender)
        {
            return Ok(());
        }
        let failed = |err: zbus::Error| fdo::Error::Failed(format!("asking polkit failed: {err}"));
        let authority = AuthorityProxy::new(connection).await.map_err(failed)?;
        let subject = (
            "system-bus-name",
            HashMap::from([("name", Value::from(sender))]),
        );
        // Flags 0: no interaction. The action allows active local sessions outright.
        let (authorized, _challenge, _details) = authority
            .check_authorization(&subject, POLKIT_ACTION, &HashMap::new(), 0, "")
            .await
            .map_err(failed)?;
        if authorized {
            info!(sender, "authorised by polkit");
            granted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(sender.to_string());
            Ok(())
        } else {
            warn!(sender, "refused by polkit");
            Err(fdo::Error::AccessDenied(format!(
                "polkit does not allow {POLKIT_ACTION} for this caller: the helper serves \
                 local, active sessions"
            )))
        }
    }
}

/// The object at [`DBUS_PATH`].
pub struct Service {
    helper: Arc<Mutex<Helper>>,
    authorizer: Authorizer,
    last_call: Arc<Mutex<Instant>>,
}

impl Service {
    pub fn new(helper: Helper, authorizer: Authorizer) -> Self {
        Self {
            helper: Arc::new(Mutex::new(helper)),
            authorizer,
            last_call: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// When the last call came, for the idle exit.
    pub fn last_call(&self) -> Arc<Mutex<Instant>> {
        Arc::clone(&self.last_call)
    }

    async fn answer(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        request: Request,
    ) -> fdo::Result<String> {
        *self.last_call.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let sender = header.sender().map(|s| s.as_str());
        self.authorizer.check(sender, connection).await?;
        let response: Response = self
            .helper
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(&request);
        serde_json::to_string(&response).map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

#[interface(name = "io.github.kywa92.KWinfo.Helper1")]
impl Service {
    async fn hello(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        protocol: u32,
    ) -> fdo::Result<String> {
        self.answer(&header, connection, Request::Hello { protocol })
            .await
    }

    async fn capabilities(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<String> {
        self.answer(&header, connection, Request::Capabilities)
            .await
    }

    async fn enumerate(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<String> {
        self.answer(&header, connection, Request::Enumerate).await
    }

    async fn sample(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<String> {
        self.answer(&header, connection, Request::Sample).await
    }
}

/// Owns the service name on the system bus and serves until idle.
pub fn serve(helper: Helper) -> anyhow::Result<()> {
    let service = Service::new(helper, Authorizer::polkit());
    let last_call = service.last_call();
    let _connection = zbus::blocking::connection::Builder::system()?
        .serve_at(DBUS_PATH, service)?
        .name(DBUS_NAME)?
        .build()?;
    info!("serving {DBUS_NAME} on the system bus");
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let idle = last_call
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        if idle >= IDLE_EXIT {
            info!("no calls for {} s; exiting", idle.as_secs());
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kwinfo_ipc::{DBUS_INTERFACE, PROTOCOL_VERSION};
    use std::os::unix::net::UnixStream;

    /// The service over a peer-to-peer connection, as the system bus would carry it.
    fn connect() -> (zbus::blocking::Connection, zbus::blocking::Connection) {
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_end)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at(
                    DBUS_PATH,
                    Service::new(Helper::new(Vec::new()), Authorizer::AllowAll),
                )
                .unwrap()
                .build()
                .unwrap()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_end)
            .p2p()
            .build()
            .unwrap();
        (server.join().unwrap(), client)
    }

    #[test]
    fn methods_answer_with_the_responses_json() {
        let (_server, client) = connect();
        let proxy =
            zbus::blocking::Proxy::new(&client, DBUS_NAME, DBUS_PATH, DBUS_INTERFACE).unwrap();
        let hello: String = proxy.call("Hello", &(PROTOCOL_VERSION,)).unwrap();
        assert!(matches!(
            serde_json::from_str::<Response>(&hello).unwrap(),
            Response::Hello {
                protocol: PROTOCOL_VERSION,
                ..
            }
        ));
        let samples: String = proxy.call("Sample", &()).unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&samples).unwrap(),
            Response::Samples {
                samples: Vec::new()
            }
        );
        let caps: String = proxy.call("Capabilities", &()).unwrap();
        assert!(caps.contains("\"type\":\"capabilities\""), "{caps}");
    }

    #[test]
    fn polkit_refuses_calls_without_a_sender() {
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_end)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at(
                    DBUS_PATH,
                    Service::new(Helper::new(Vec::new()), Authorizer::polkit()),
                )
                .unwrap()
                .build()
                .unwrap()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_end)
            .p2p()
            .build()
            .unwrap();
        let _server = server.join().unwrap();
        let proxy =
            zbus::blocking::Proxy::new(&client, DBUS_NAME, DBUS_PATH, DBUS_INTERFACE).unwrap();
        let err = proxy.call::<_, _, String>("Sample", &()).unwrap_err();
        let zbus::Error::MethodError(name, _, _) = err else {
            panic!("{err:?}");
        };
        assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.AccessDenied");
    }
}
