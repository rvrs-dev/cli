//! The BGP tool: the route an address falls in, who announces it, whether RPKI authorizes that,
//! how route collectors and the user's looking glasses reach it, and the network behind it.

pub mod glass;
pub mod glasses;
pub mod lookup;
pub mod result;
pub mod ripestat;

pub use glass::{GlassKind, GlassPath, GlassResult, GlassRoute, GlassSessions, LookingGlass};
pub use glasses::Glasses;
pub use lookup::{BgpEvent, Folded, NetworkPart};
pub use result::{
    Activity, Address, BgpNetwork, BgpResult, Change, ChangeKind, Check, Grade, Neighbor, Origin,
    Path, RelatedPrefix, Roa, RpkiStatus, Share, Side, Source, Span, Target, TargetKind,
    TraceComparison, Validation, Verdict,
};
