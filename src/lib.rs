//! An unofficial, blocking client for the AWS Lambda Runtime API.
//!
//! [`Runtime::next`] hands over one [`Invocation`] as a value, and the caller decides when and how to answer it:
//! a byte-exact [`respond`](Invocation::respond), a [`stream`](Invocation::stream) whose head is on the socket before
//! the call returns, or a [`fail`](Invocation::fail). Each answer consumes the invocation. There is no handler loop
//! and no async runtime; the crate runs on `std` sockets.
//!
//! ```no_run
//! use lambda_runtime_base::{Result, Runtime};
//!
//! fn main() -> Result<()> {
//!     let runtime = Runtime::from_env()?;
//!
//!     loop {
//!         let invocation = runtime.next()?;
//!         let request = invocation.body().to_vec();
//!
//!         // Answer in one of three ways; each consumes the invocation.
//!         match request.first() {
//!             Some(b's') => {
//!                 let mut stream = invocation.stream()?;
//!                 stream.send(b"first")?;
//!                 stream.send(b"second")?;
//!                 stream.finish()?;
//!             }
//!             Some(_) => invocation.respond(&request)?,
//!             None => invocation.fail("Function.EmptyEvent", "the event is empty")?,
//!         }
//!     }
//! }
//! ```
//!
//! # Errors
//!
//! [`Error`] is split by what the caller does next: a broken environment ends the process, while a rejected answer
//! or a bad payload ends one invocation at most.

mod error;
mod failure;
mod http;
mod invocation;
mod protocol;
mod runtime;
mod stream;

pub use error::{Error, Result};
pub use invocation::Invocation;
pub use runtime::Runtime;
pub use stream::Stream;
