//! The `stdio` transport: one JSON request per line on standard input, one JSON response
//! per line on standard output, until standard input closes. KWinfo starts the helper
//! this way through `pkexec` when no system service is installed. Logs go to standard
//! error, which KWinfo reads into its own log.

use std::io::{self, BufRead, Write};

use kwinfo_ipc::framing::{read_message, write_message};
use kwinfo_ipc::{Request, Response};
use tracing::{debug, warn};

use crate::server::Helper;

/// Serves requests from `input` until it ends.
pub fn serve(
    helper: &mut Helper,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    loop {
        let request = match read_message::<_, Request>(&mut input) {
            Ok(Some(request)) => request,
            Ok(None) => {
                debug!("standard input closed; exiting");
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                warn!("bad request: {err}");
                write_message(
                    &mut output,
                    &Response::Error {
                        message: format!("bad request: {err}"),
                    },
                )?;
                continue;
            }
            Err(err) => return Err(err),
        };
        let response = helper.handle(&request);
        write_message(&mut output, &response)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kwinfo_ipc::PROTOCOL_VERSION;

    #[test]
    fn answers_line_by_line_and_reports_bad_lines() {
        let mut helper = Helper::new(Vec::new());
        let input = format!(
            "{}\nnot json\n\n{}\n",
            serde_json::to_string(&Request::Hello {
                protocol: PROTOCOL_VERSION
            })
            .unwrap(),
            serde_json::to_string(&Request::Sample).unwrap()
        );
        let mut output = Vec::new();
        serve(&mut helper, input.as_bytes(), &mut output).unwrap();
        let lines: Vec<Response> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "the blank line gets no answer");
        assert!(matches!(lines[0], Response::Hello { .. }));
        assert!(
            matches!(&lines[1], Response::Error { message } if message.starts_with("bad request"))
        );
        assert_eq!(
            lines[2],
            Response::Samples {
                samples: Vec::new()
            }
        );
    }
}
