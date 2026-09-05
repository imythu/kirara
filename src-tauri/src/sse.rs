/// Incrementally decode SSE without buffering the lifetime of a log stream.
#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    event: String,
    data: Vec<String>,
    event_bytes: usize,
    after_cr: bool,
}

const MAX_EVENT_BYTES: usize = 256 * 1024;

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        let mut messages = Vec::new();
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.finish_line(&mut messages)?;
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.event_bytes + self.line.len() > MAX_EVENT_BYTES {
                    return Err("日志事件超过大小限制".into());
                }
            }
        }
        Ok(messages)
    }

    fn finish_line(&mut self, messages: &mut Vec<String>) -> Result<(), String> {
        let line =
            String::from_utf8(std::mem::take(&mut self.line)).map_err(|error| error.to_string())?;
        if line.is_empty() {
            if self.event == "log" && !self.data.is_empty() {
                messages.push(self.data.join("\n"));
            }
            self.event.clear();
            self.data.clear();
            self.event_bytes = 0;
        } else if !line.starts_with(':') {
            self.event_bytes += line.len();
            let (field, value) = line.split_once(':').unwrap_or((&line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = value.into(),
                "data" => self.data.push(value.into()),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_utf8_crlf_and_multiline_data() {
        let mut decoder = Decoder::default();
        let input = ": heartbeat\r\nevent: log\r\ndata: 中文\r\ndata: second line\r\n\r\n";
        let mut events = Vec::new();
        for byte in input.as_bytes() {
            events.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(events, ["中文\nsecond line"]);
    }

    #[test]
    fn events_are_isolated_and_comments_are_ignored() {
        let mut decoder = Decoder::default();
        assert_eq!(
            decoder.push(b"event: log\ndata: first\n\n: alive\n\ndata: ignored\n\nevent: log\ndata: second\n\n").unwrap(),
            ["first", "second"]
        );
        assert!(
            decoder
                .push(b"event: log\ndata: unfinished")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_an_unbounded_event() {
        let mut decoder = Decoder::default();
        assert!(decoder.push(&vec![b'x'; MAX_EVENT_BYTES + 1]).is_err());
        let mut decoder = Decoder::default();
        for _ in 0..MAX_EVENT_BYTES / 8 {
            if decoder.push(b"data: x\n").is_err() {
                return;
            }
        }
        assert!(decoder.push(&vec![b'x'; MAX_EVENT_BYTES]).is_err());
    }
}
