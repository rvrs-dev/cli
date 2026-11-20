//! DNS: the wire format, one question to one server, and the lookup that asks this device's
//! resolver, public resolvers and the zone's own servers, with what follows from their answers.
//! No call to a system resolver or tool: rvrs asks every server itself.

pub mod client;
pub mod lookup;
pub mod message;
pub mod result;

pub use lookup::{DnsEvent, Options, Resolver};
pub use message::{type_code, type_name, types, Record, Section};
pub use result::{DelegationStep, Diff, DnsResult, Dnssec, SourceKind, SourceResult};
