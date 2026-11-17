//! A fake Runtime API: each test scripts the server side of its connections on a thread.

#![allow(dead_code)] // each test crate uses a different subset

use lambda_runtime_base::Runtime;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

pub const ACCEPTED: &[u8] = b"HTTP/1.1 202 Accepted\r\nContent-Length: 15\r\n\r\n{\"status\":\"OK\"}";

/// A runtime pointed at a fresh listener on a random local port.
pub fn runtime() -> (Runtime, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let runtime = Runtime::new(listener.local_addr().unwrap().to_string());

    (runtime, listener)
}

/// A `/next` response with `headers` and a `Content-Length` body.
pub fn next(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut response = String::from("HTTP/1.1 200 OK\r\n");

    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }

    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));

    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    response
}

/// An error response with the Runtime API's JSON body.
pub fn rejected(status: u16, error_type: &str) -> Vec<u8> {
    let body = format!(r#"{{"errorMessage":"State transition is not allowed","errorType":"{error_type}"}}"#);

    format!(
        "HTTP/1.1 {status} Forbidden\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The server side of one connection.
pub struct Peer {
    reader: BufReader<TcpStream>,
}

impl Peer {
    pub fn accept(listener: &TcpListener) -> Self {
        let (stream, _) = listener.accept().unwrap();

        Self {
            reader: BufReader::new(stream),
        }
    }

    /// Reads a request head, leaving the body unread.
    pub fn head(&mut self) -> Request {
        let mut lines = Vec::new();

        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            assert!(line.ends_with("\r\n"), "head ended early: {lines:?}");

            if line == "\r\n" {
                break;
            }

            lines.push(line.trim_end().to_owned());
        }

        let line = lines.remove(0);
        let headers = lines
            .into_iter()
            .map(|line| {
                let (name, value) = line.split_once(": ").unwrap();
                (name.to_owned(), value.to_owned())
            })
            .collect();

        Request {
            line,
            headers,
            body: Vec::new(),
        }
    }

    /// Reads a request with a `Content-Length` body.
    pub fn request(&mut self) -> Request {
        let mut request = self.head();
        let length: usize = request.header("Content-Length").unwrap().parse().unwrap();

        request.body = vec![0; length];
        self.reader.read_exact(&mut request.body).unwrap();
        request
    }

    /// Reads a chunked body as raw bytes, through the blank line after the last chunk and its trailers.
    pub fn chunked(&mut self) -> Vec<u8> {
        let mut raw = Vec::new();

        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            raw.extend_from_slice(line.as_bytes());

            let size = usize::from_str_radix(line.trim_end(), 16).unwrap();

            if size == 0 {
                break;
            }

            let mut data = vec![0; size + 2];
            self.reader.read_exact(&mut data).unwrap();
            raw.extend_from_slice(&data);
        }

        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            raw.extend_from_slice(line.as_bytes());

            if line == "\r\n" {
                return raw;
            }
        }
    }

    pub fn reply(&mut self, response: &[u8]) {
        self.reader.get_mut().write_all(response).unwrap();
    }

    /// Writes the response one byte per write, so every boundary falls between reads.
    pub fn reply_slowly(&mut self, response: &[u8]) {
        let stream = self.reader.get_mut();
        stream.set_nodelay(true).unwrap();

        for byte in response {
            stream.write_all(&[*byte]).unwrap();
            stream.flush().unwrap();
        }
    }
}

/// One request as the client sent it
#[derive(Debug)]
pub struct Request {
    pub line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
