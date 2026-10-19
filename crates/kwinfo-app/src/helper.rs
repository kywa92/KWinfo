//! The privileged helper in the UI (PLAN.md 3.5): the toolbar's status indicator and the
//! Settings → Helper tab, which installs or uninstalls the system service and starts a
//! helper for the session. `pkexec` runs on a background thread, since it waits for the
//! user to type a password.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::{Button, Color32, RichText, Spinner, Ui};
use kwinfo_diag::action;
use kwinfo_ipc::LOCAL_HELPER_PATH;
use kwinfo_ipc::client::{self, HelperState, SessionCommand, Via};

use crate::app::KwinfoApp;
use crate::theme::{GRAPHIC_CONTRAST, ThemeTokens};

/// An install or uninstall waiting for `pkexec` to finish.
pub(crate) struct HelperJob {
    what: &'static str,
    rx: Receiver<Result<String, String>>,
}

impl HelperJob {
    fn start(
        what: &'static str,
        helper: PathBuf,
        args: Vec<&'static str>,
        ctx: egui::Context,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("kwinfo-helper-{what}"))
            .spawn(move || {
                let _ = tx.send(client::pkexec(&helper, &args));
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            tracing::warn!("could not start the {what} job: {err}");
        }
        Self { what, rx }
    }

    fn poll(&self) -> Option<Result<String, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("the job ended without an answer".to_string()))
            }
        }
    }
}

/// The indicator's dot colour and accessible name for a state.
pub(crate) fn indicator(state: &HelperState, tokens: &ThemeTokens) -> (Color32, &'static str) {
    let visible = |color: Color32| tokens.readable_on(color, &[tokens.window_bg], GRAPHIC_CONTRAST);
    match state {
        HelperState::Running { .. } => (visible(tokens.graph.fan), "Helper: running"),
        HelperState::Connecting | HelperState::Authenticating => {
            (visible(tokens.accent), "Helper: starting")
        }
        HelperState::NotInstalled => (tokens.text_secondary, "Helper: not installed"),
        HelperState::Denied(_) | HelperState::Failed(_) => {
            (visible(tokens.highlight), "Helper: problem")
        }
        HelperState::Unavailable(_) => (tokens.text_secondary, "Helper: not used"),
    }
}

impl KwinfoApp {
    /// Picks up a finished install or uninstall and looks for the service again.
    pub(crate) fn poll_helper_job(&mut self) {
        let Some(result) = self.helper_job.as_ref().and_then(HelperJob::poll) else {
            return;
        };
        let what = self.helper_job.take().map_or("job", |job| job.what);
        match &result {
            Ok(_) => action!("helper.done", job = what, ok = true),
            Err(err) => {
                action!("helper.done", job = what, ok = false);
                tracing::warn!("helper {what} failed: {err}");
            }
        }
        self.helper_result = Some((what, result));
        self.helper.retry();
    }

    /// An install or uninstall is waiting for `pkexec`.
    pub fn helper_busy(&self) -> bool {
        self.helper_job.is_some()
    }

    /// The last install or uninstall's output, or what went wrong.
    pub fn helper_result(&self) -> Option<&Result<String, String>> {
        self.helper_result.as_ref().map(|(_, result)| result)
    }

    /// `pkexec kwinfo-helper install`: copies the helper under /usr/local and sets up the
    /// system service, which KWinfo then uses at every start.
    pub(crate) fn install_helper(&mut self, ctx: &egui::Context) {
        let Some(helper) = self.helper_binary.clone() else {
            return;
        };
        action!("helper.install", helper = %helper.display());
        match client::runnable_by_root(&helper) {
            Ok(path) => {
                self.helper_job = Some(HelperJob::start(
                    "install",
                    path,
                    vec!["install"],
                    ctx.clone(),
                ))
            }
            Err(err) => {
                self.helper_result = Some((
                    "install",
                    Err(format!("The helper could not be prepared: {err}")),
                ));
            }
        }
    }

    /// `pkexec /usr/local/libexec/kwinfo/kwinfo-helper uninstall`.
    pub(crate) fn uninstall_helper(&mut self, ctx: &egui::Context) {
        action!("helper.uninstall");
        self.helper_job = Some(HelperJob::start(
            "uninstall",
            PathBuf::from(LOCAL_HELPER_PATH),
            vec!["uninstall"],
            ctx.clone(),
        ));
    }

    /// `pkexec kwinfo-helper stdio` for this session.
    pub(crate) fn start_session_helper(&mut self) {
        let Some(helper) = self.helper_binary.clone() else {
            return;
        };
        action!("helper.session_start");
        match client::runnable_by_root(&helper) {
            Ok(path) => self.helper.start_session(SessionCommand::pkexec(&path)),
            Err(err) => {
                self.helper_result = Some((
                    "session",
                    Err(format!("The helper could not be prepared: {err}")),
                ));
            }
        }
    }

    pub(crate) fn helper_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        let ctx = ui.ctx().clone();
        let state = self.helper.state();
        ui.label(RichText::new(state.describe()).strong());
        if let HelperState::Running { capabilities, .. } = &state {
            for note in &capabilities.notes {
                ui.label(RichText::new(format!("• {note}")).color(tokens.text_secondary));
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "The helper reads, as root, what the kernel keeps from other users: CPU \
                 package, core, GT and DRAM power (RAPL), core voltages, power limits and \
                 package C-states (MSRs), S.M.A.R.T. data (through smartctl) and GPU engine \
                 load. It never writes to the hardware.",
            )
            .small()
            .color(tokens.text_secondary),
        );
        ui.add_space(8.0);
        if matches!(state, HelperState::Unavailable(_)) {
            return;
        }

        let packaged = client::installed_by_package();
        let local = client::installed_locally();
        let busy = self.helper_busy() || matches!(state, HelperState::Authenticating);
        let found = self.helper_binary.is_some();
        let running_via = match &state {
            HelperState::Running { via, .. } => Some(*via),
            _ => None,
        };
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let offer_install =
                    !packaged && !local && found && running_via != Some(Via::Service);
                if offer_install
                    && ui
                        .button("Install helper…")
                        .on_hover_text(
                            "Copies the helper to /usr/local/libexec/kwinfo and sets up the system \
                             service; asks for an administrator password once. KWinfo then uses it \
                             at every start.",
                        )
                        .clicked()
                {
                    self.install_helper(&ctx);
                }
                if running_via.is_none()
                    && found
                    && ui
                        .button("Run for this session")
                        .on_hover_text(
                            "Starts the helper until KWinfo quits; asks for an administrator \
                             password every time.",
                        )
                        .clicked()
                {
                    self.start_session_helper();
                }
                if running_via == Some(Via::Session) && ui.button("Stop session helper").clicked() {
                    action!("helper.session_stop");
                    self.helper.stop_session();
                }
                if running_via.is_none() && ui.button("Try again").clicked() {
                    action!("helper.retry");
                    self.helper.retry();
                }
                if local
                    && ui
                        .add(Button::new("Uninstall helper…"))
                        .on_hover_text("Removes the system service and the helper from /usr/local")
                        .clicked()
                {
                    self.uninstall_helper(&ctx);
                }
            });
        });
        if busy {
            ui.horizontal(|ui| {
                ui.add(Spinner::new());
                ui.label("Waiting for the administrator password…");
            });
        } else if let Some((what, result)) = &self.helper_result {
            ui.add_space(4.0);
            match result {
                Ok(output) => {
                    let done = match *what {
                        "install" => "The helper is installed.",
                        "uninstall" => "The helper is uninstalled.",
                        _ => "Done.",
                    };
                    ui.label(RichText::new(done).strong());
                    let last = output.lines().rev().take(4).collect::<Vec<_>>();
                    for line in last.into_iter().rev() {
                        ui.label(
                            RichText::new(line)
                                .small()
                                .monospace()
                                .color(tokens.text_secondary),
                        );
                    }
                }
                Err(err) => {
                    ui.label(RichText::new(err).color(tokens.text_primary));
                }
            }
        }
        ui.add_space(8.0);
        let footnote = if packaged {
            "The helper was installed by a package; remove the package to remove it.".to_string()
        } else {
            match &self.helper_binary {
                Some(path) => format!("Helper program: {}", path.display()),
                None => "kwinfo-helper was not found next to KWinfo, so it can be neither installed nor \
                         run from here; the packages and the AppImage include it."
                    .to_string(),
            }
        };
        ui.label(RichText::new(footnote).small().color(tokens.text_secondary));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT, contrast};
    use kwinfo_ipc::Capabilities;

    #[test]
    fn every_state_has_a_name_and_a_visible_dot() {
        let states = [
            HelperState::Running {
                via: Via::Service,
                version: "0.1.0".into(),
                capabilities: Capabilities::default(),
            },
            HelperState::Connecting,
            HelperState::Authenticating,
            HelperState::NotInstalled,
            HelperState::Denied("no".into()),
            HelperState::Failed("no".into()),
            HelperState::Unavailable("mock".into()),
        ];
        for tokens in [&DARK, &LIGHT] {
            for state in &states {
                let (color, name) = indicator(state, tokens);
                assert!(name.starts_with("Helper: "));
                assert!(
                    contrast(color, tokens.window_bg) >= GRAPHIC_CONTRAST,
                    "{name} in {:?}",
                    tokens.palette
                );
            }
        }
    }
}
