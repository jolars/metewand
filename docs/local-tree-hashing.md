# Local-tree hashing

`metewand-runtime` hashes filesystem trees without admitting host paths or
incidental filesystem metadata into their content digest:

```rust
use std::path::Path;

use metewand_runtime::local_tree::hash_local_tree;

let digest = hash_local_tree(Path::new("prepared-artifact"))?;
assert_eq!(digest.bytes().len(), 32);
# Ok::<(), metewand_runtime::local_tree::LocalTreeHashError>(())
```

The root is canonicalized and must be a directory, but the root's name and
spelling are not hashed. Every descendant path must be UTF-8. The hasher builds
paths from directory-entry names with `/` separators, rejects backslashes, and
sorts the complete inventory lexicographically by UTF-8 bytes. Empty
directories are entries.

Regular files contribute their complete bytes and one executable flag. On Unix,
the flag is true when any of the owner, group, or other executable bits is set;
the identity of the particular bit is irrelevant. Platforms without Unix mode
bits contribute false. Symbolic links contribute their exact UTF-8 target
spelling and are never followed, so hashing also works for broken links. Link
containment and traversal safety remain validation concerns for the caller.
Directories, regular files, and symbolic links are the only supported entry
types. Sockets, FIFOs, and device nodes are rejected.

Timestamps, ownership, the read-only bit, and all non-executable permission
bits are deliberately absent. A file whose size changes while its contents are
read is rejected instead of receiving a digest for an inconsistent transcript.

## Version-1 transcript

`LOCAL_TREE_HASH_VERSION` is `1`. Its transcript is prefix-free and uses these
primitives:

- `u32(n)` and `u64(n)` are unsigned big-endian integers;
- `bytes(value)` is `u64(value.len) || value`; and
- `path` and symbolic-link target values are their UTF-8 bytes.

The SHA-256 input is:

```text
"metewand-local-tree\0"
|| u32(LOCAL_TREE_HASH_VERSION)
|| u64(entry_count)
|| entry[0]
|| ...
|| entry[entry_count - 1]
```

Entries appear in normalized path order and have one of these forms:

```text
directory = bytes(path) || "d"
file      = bytes(path) || "f" || executable_u8 || u64(size) || file_bytes
symlink   = bytes(path) || "l" || bytes(target)
```

`executable_u8` is exactly `0x00` or `0x01`. The root itself is omitted. The
empty tree therefore consists only of the domain, version, and a zero entry
count. Changing this transcript requires a deliberate compatibility-version
review and new golden vectors.
