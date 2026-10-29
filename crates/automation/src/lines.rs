//! Bounded newline-delimited input shared by the JSON-lines and MCP
//! transports. Oversized lines are consumed in bounded space and reported,
//! so a client cannot exhaust memory with an endless line.
use std::io::BufRead;

pub enum Line {
    Text(String),
    TooLarge,
    NotUtf8,
}

/// Read the next line without its terminator, or `None` at end of input.
pub fn next(input: &mut impl BufRead, limit: usize) -> std::io::Result<Option<Line>> {
    let mut line = Vec::new();
    let mut oversized = false;
    let mut received = false;
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let (count, newline) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        let content = count - usize::from(newline);
        if !oversized {
            if line.len().saturating_add(content) > limit {
                oversized = true;
                line.clear();
            } else if let Some(bytes) = chunk.get(..content) {
                line.extend_from_slice(bytes);
            }
        }
        input.consume(count);
        received = true;
        if newline {
            break;
        }
    }
    if !received {
        return Ok(None);
    }
    if oversized {
        return Ok(Some(Line::TooLarge));
    }
    Ok(Some(match String::from_utf8(line) {
        Ok(text) => Line::Text(text),
        Err(_) => Line::NotUtf8,
    }))
}
