//! One JSON message per line, for the `stdio` transport.

use std::io::{self, BufRead, Read as _, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The longest line accepted; a full enumeration of a large machine is far smaller.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// Writes `message` as one line and flushes.
pub fn write_message<T: Serialize>(out: &mut impl Write, message: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// Reads the next message. `Ok(None)` at the end of the input; blank lines are skipped;
/// a line that is not a message is an `InvalidData` error, after which reading can go on.
pub fn read_message<R: BufRead, T: DeserializeOwned>(input: &mut R) -> io::Result<Option<T>> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = input
            .by_ref()
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(None);
        }
        if line.len() > MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message too long",
            ));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        return serde_json::from_slice(&line)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;

    #[test]
    fn messages_round_trip_one_per_line() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &Request::Sample).unwrap();
        write_message(&mut buffer, &Request::Enumerate).unwrap();
        assert_eq!(buffer.iter().filter(|&&b| b == b'\n').count(), 2);
        let mut input = buffer.as_slice();
        assert_eq!(
            read_message::<_, Request>(&mut input).unwrap(),
            Some(Request::Sample)
        );
        assert_eq!(
            read_message::<_, Request>(&mut input).unwrap(),
            Some(Request::Enumerate)
        );
        assert_eq!(read_message::<_, Request>(&mut input).unwrap(), None);
    }

    #[test]
    fn bad_lines_are_errors_and_reading_goes_on() {
        let mut input = "garbage\n\n{\"type\":\"sample\"}\n".as_bytes();
        let err = read_message::<_, Request>(&mut input).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            read_message::<_, Request>(&mut input).unwrap(),
            Some(Request::Sample)
        );
    }
}
