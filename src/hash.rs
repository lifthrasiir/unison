//! The crate's `HashMap` and `HashSet`: `std`'s, with the hasher swapped.
//!
//! Nearly every map here is keyed by a glyph name — a short ASCII string — and
//! is read far more often than it is written: the expansion, the build, the
//! validation and the specimen each walk tens of thousands of names through
//! several maps apiece. `std`'s default is SipHash-1-3, keyed at random for
//! HashDoS resistance, and against that traffic it was the single largest
//! entry in a rebuild's profile: about a fifth of the serial time, spread thin
//! enough over every stage that no one call site looked worth fixing.
//!
//! There is no attacker here — the keys are the names in the author's own
//! source files — so the resistance buys nothing and the speed is worth having.
//! `rustc-hash` is the hash the compiler uses for the same reason and on the
//! same kind of key.
//!
//! Two consequences worth knowing:
//!
//! - The hasher has **no random seed**, so iteration order is stable from run
//!   to run. That is not a licence to depend on it — it is still arbitrary, and
//!   the goldens exist to catch anything that starts to — but it does mean a
//!   reordering shows up the same way twice rather than flickering.
//! - `HashMap::new` is only defined for the default hasher, so these are built
//!   with `HashMap::default()` and `HashMap::with_capacity_and_hasher(n, …)`.
//!
//! Where a map is *not* keyed by author-supplied text and not hot — anything
//! reading untrusted input, should such a thing ever appear — reach for
//! `crate::hash::HashMap` by its full path and say why.

pub type HashMap<K, V> = std::collections::HashMap<K, V, rustc_hash::FxBuildHasher>;
pub type HashSet<T> = std::collections::HashSet<T, rustc_hash::FxBuildHasher>;
