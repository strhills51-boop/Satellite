//! Integer identities. Entities are addressed by id, never by name (Satellite rule `stable-id-only`). @sat:ids
//!
//! Owns: typed integer ids of the warm and hot tiers.

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[repr(transparent)]
        pub struct $name(pub u32);

        impl $name {
            #[inline]
            pub fn idx(self) -> usize {
                self.0 as usize
            }
            #[inline]
            pub fn from_idx(i: usize) -> Self {
                debug_assert!(i <= u32::MAX as usize);
                Self(i as u32)
            }
        }
    };
}

id_type!(/// A mod (or pseudo-mod such as Overwrite) inside one provider table.
    ModId);
id_type!(/// A distinct virtual path (one path group of the provider table).
    PathId);
id_type!(/// An interned directory.
    DirId);
id_type!(/// An interned path segment.
    NameId);
id_type!(/// An exact overwrite chain inside one projection.
    ChainId);
id_type!(/// A connected component of mods linked by chains inside one projection.
    ContextId);
id_type!(/// An archive (BSA/BA2) contributing virtual paths.
    ArchiveId);
