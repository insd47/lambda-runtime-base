use crate::error::{Error, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

const MAX_HEADERS: usize = 64;

/// One HTTP/1.1 exchange with the Runtime API on its own connection.
///
/// - Every request carries `Connection: close`; nothing is pooled, so a stream never shares its socket.
/// - Writes go straight to the socket with Nagle's algorithm off, so each write leaves the process when it returns.
/// - Reads have no timeout: `/next` blocks while Lambda freezes the environment between invocations.
pub struct Connection {
    reader: BufReader<TcpStream>,
}

impl Connection {
    /// Sends a request with a `Content-Length` body and reads the response.
    pub fn send(endpoint: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let length = body.len().to_string();
        let mut all = vec![("Content-Length", length.as_str())];
        all.extend_from_slice(headers);

        let head = Self::head(endpoint, method, path, &all)?;
        let mut connection = Self::open(endpoint)?;

        connection.write(head.as_bytes())?;
        connection.write(body)?;
        connection.response()
    }

    /// Opens a request whose body follows in chunks, returning once the head is on the socket.
    pub fn start(endpoint: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> Result<Self> {
        let head = Self::head(endpoint, method, path, headers)?;
        let mut connection = Self::open(endpoint)?;

        connection.write(head.as_bytes())?;
        Ok(connection)
    }

    /// Ends a chunked body with the last chunk and `trailers` in one write.
    pub fn end(&mut self, trailers: &[(&str, &str)]) -> Result<()> {
        let fields = Self::fields(trailers)?;

        self.write(format!("0\r\n{fields}").as_bytes())
    }

    /// Writes `bytes` to the socket.
    ///
    /// - The Runtime API may answer and close before the request ends, such as a 403 to a stream or a 413 to a large
    ///   body. When a write fails, an error answer already received is returned instead of the broken pipe.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let Err(error) = self.reader.get_mut().write_all(bytes) else {
            return Ok(());
        };

        if let Ok(response) = self.response() {
            response.accepted()?;
        }

        Err(error.into())
    }

    fn open(endpoint: &str) -> Result<Self> {
        let stream = TcpStream::connect(endpoint)?;
        stream.set_nodelay(true)?;

        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    /// The request line and headers, built before connecting so an invalid header sends nothing.
    fn head(endpoint: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> Result<String> {
        let fields = Self::fields(headers)?;

        Ok(format!(
            "{method} {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n{fields}"
        ))
    }

    /// Header lines and the blank line after them.
    ///
    /// - A value with a control character could end its line early and inject a header, so it fails with
    ///   [`Error::Header`] before anything is written.
    fn fields(headers: &[(&str, &str)]) -> Result<String> {
        let mut fields = String::new();

        for (name, value) in headers {
            if value.chars().any(char::is_control) {
                return Err(Error::Header(value.to_string()));
            }

            fields.push_str(&format!("{name}: {value}\r\n"));
        }

        fields.push_str("\r\n");
        Ok(fields)
    }

    pub fn response(&mut self) -> Result<Response> {
        let head = self.read_head()?;
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut parsed = httparse::Response::new(&mut headers);

        let complete = parsed
            .parse(&head)
            .map_err(|error| Error::Protocol(error.to_string()))?;
        complete
            .is_complete()
            .then_some(())
            .ok_or_else(|| protocol("incomplete response head"))?;

        let status = parsed.code.ok_or_else(|| protocol("response without a status code"))?;
        let headers = parsed
            .headers
            .iter()
            .map(|header| {
                let value = std::str::from_utf8(header.value).map_err(|_| protocol("header value is not UTF-8"))?;

                Ok((header.name.to_owned(), value.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut response = Response {
            status,
            headers,
            body: Vec::new(),
        };

        response.body = self.read_body(&response)?;
        Ok(response)
    }

    /// Reads up to and including the blank line that ends the head.
    fn read_head(&mut self) -> Result<Vec<u8>> {
        let mut head = Vec::new();

        loop {
            let start = head.len();

            if self.reader.read_until(b'\n', &mut head)? == 0 {
                return Err(protocol("connection closed before the response head ended"));
            }

            if start > 0 && &head[start..] == b"\r\n" {
                return Ok(head);
            }
        }
    }

    fn read_body(&mut self, response: &Response) -> Result<Vec<u8>> {
        let chunked = response
            .header("Transfer-Encoding")
            .is_some_and(|encoding| encoding.to_ascii_lowercase().contains("chunked"));

        if chunked {
            return self.read_chunked();
        }

        let mut body = Vec::new();

        match response.header("Content-Length") {
            Some(length) => {
                let length: u64 = length.trim().parse().map_err(|_| protocol("invalid Content-Length"))?;
                self.read_exact_into(length, &mut body)?;
            }
            None => {
                self.reader.read_to_end(&mut body)?;
            }
        }

        Ok(body)
    }

    fn read_chunked(&mut self) -> Result<Vec<u8>> {
        let mut body = Vec::new();

        loop {
            let line = self.read_line()?;
            let size = line.split(';').next().unwrap_or_default().trim();
            let size = u64::from_str_radix(size, 16).map_err(|_| protocol("invalid chunk size"))?;

            if size == 0 {
                break;
            }

            self.read_exact_into(size, &mut body)?;

            if !self.read_line()?.is_empty() {
                return Err(protocol("chunk data longer than its size"));
            }
        }

        // Trailers carry nothing this crate reads; skip them up to the blank line.
        while !self.read_line()?.is_empty() {}

        Ok(body)
    }

    /// Reads one CRLF-terminated line without its terminator.
    fn read_line(&mut self) -> Result<String> {
        let mut line = Vec::new();

        if self.reader.read_until(b'\n', &mut line)? == 0 {
            return Err(protocol("connection closed inside the body"));
        }

        let line = line
            .strip_suffix(b"\r\n")
            .ok_or_else(|| protocol("line without CRLF"))?;

        String::from_utf8(line.to_vec()).map_err(|_| protocol("line is not UTF-8"))
    }

    fn read_exact_into(&mut self, length: u64, body: &mut Vec<u8>) -> Result<()> {
        let read = (&mut self.reader).take(length).read_to_end(body)?;

        if read as u64 != length {
            return Err(protocol("connection closed inside the body"));
        }

        Ok(())
    }
}

/// A complete response from the Runtime API.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// The first value of a header, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Passes a 2xx response through and turns any other status into [`Error::Rejected`].
    pub fn accepted(self) -> Result<Self> {
        if (200..300).contains(&self.status) {
            return Ok(self);
        }

        let body: Option<serde_json::Value> = serde_json::from_slice(&self.body).ok();
        let field = |name: &str| {
            let value = body.as_ref()?.get(name)?.as_str()?;

            Some(value.to_owned())
        };

        Err(Error::Rejected {
            status: self.status,
            error_type: field("errorType"),
            message: field("errorMessage"),
        })
    }
}

fn protocol(reason: &str) -> Error {
    Error::Protocol(reason.to_owned())
}
