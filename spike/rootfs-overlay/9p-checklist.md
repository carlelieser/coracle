# 9p requirements for serving an overlayfs lower layer

What a 9p server must provide for the Linux overlayfs driver to accept it as
`lowerdir`. M3's gates consume this list verbatim against our JS-side server.

Kernel: Linux 6.12.107. Paths below are relative to that tree.

**Evidence grades.** `OBSERVED` — the spike ran it and the behavior is in
`RESULTS.md`. `SOURCE` — read directly out of the kernel; deterministic, but the
spike did not isolate it with a failing case. `INFERRED` — reasoned from
adjacent code; weakest, flagged for M3 to settle empirically.

---

## A. Protocol baseline

### A1. Speak 9p2000.L, not 9p2000.u — REQUIRED
`SOURCE` `fs/9p/vfs_inode_dotl.c:924` — `link`, and the whole
`v9fs_dir_inode_operations_dotl` table, exist only on the `.L` path. 9p2000.u
has no `LINK`, no `GETATTR`/`SETATTR` with a Linux stat shape, and no xattr
walk. Mount with `version=9p2000.L`.

Server must implement at minimum: `TVERSION`, `TATTACH`, `TWALK`, `TLOPEN`,
`TLCREATE`, `TREAD`, `TWRITE`, `TCLUNK`, `TGETATTR`, `TSETATTR`, `TREADDIR`,
`TMKDIR`, `TUNLINKAT`, `TRENAMEAT`, `TSYMLINK`, `TREADLINK`, `TLINK`,
`TXATTRWALK`, `TXATTRCREATE`, `TSTATFS`, `TFSYNC`.

### A2. `TREADDIR` must return a real `d_type` per entry — REQUIRED
`SOURCE` `fs/9p/vfs_dir.c:185-188` — the `.L` readdir path passes
`curdirent.d_type` from the wire straight into `dir_emit()`. The kernel does
not synthesise it; whatever the server sends is what the VFS sees.

Why overlayfs cares, and it is not the obvious reason:
`SOURCE` `fs/overlayfs/readdir.c:170` — `if (d_type == DT_CHR)` is how
overlayfs collects **whiteout candidates** while reading a directory. An entry
reported as `DT_UNKNOWN` never enters `first_maybe_whiteout`, so a whiteout
living on that layer is not recognised as one.

Note the asymmetry, because it is easy to get backwards:
`SOURCE` `fs/overlayfs/super.c:667` — `ovl_check_d_type_supported()` is called
on the **workdir (upper)**, never on a lower layer, and a negative result is
only `pr_warn("upper fs needs to support d_type")`. So a `DT_UNKNOWN` lower
does not refuse the mount; it degrades silently. That makes this a
correctness requirement our tests must assert directly, not something a mount
failure will catch for us.

Serve the full set: `DT_REG`, `DT_DIR`, `DT_LNK`, `DT_CHR`, `DT_BLK`,
`DT_FIFO`, `DT_SOCK`.

### A3. `msize` large enough to carry a directory entry — REQUIRED
`INFERRED` — the spike mounted with `msize=512000` and worked, but never
tested a too-small value, so the failure mode is reasoned rather than seen. A
too-small `msize` truncates `TREADDIR` replies and caps read/write throughput.
Negotiate honestly in `RVERSION` and never return an `msize` larger than the
client asked for.

---

## B. Identity: QIDs and inode numbers

### B1. `qid.path` is the guest inode number — REQUIRED, and stronger than it looks
`SOURCE` `fs/9p/v9fs_vfs.h:48-50`

    #define QID2INO(q) ((ino_t) (((q)->path+2) ^ (((q)->path) >> 32)))   /* 32-bit ino_t */
    #define QID2INO(q) ((ino_t) ((q)->path+2))                            /* 64-bit ino_t */

`SOURCE` `fs/9p/vfs_inode_dotl.c:112` — that value is the key passed to
`iget5_locked()`. So `qid.path` *is* inode identity inside the guest.

Consequences a server implementer must honour:

- **Stable for the life of a file.** The same file reached through `TWALK`,
  `TREADDIR`, or a fresh `TATTACH` must carry the same `qid.path`. A server
  that hands out a fresh id per walk makes every lookup a different inode.
- **Unique across the served tree.** Two distinct files sharing a `qid.path`
  are aliased into one inode in the guest page cache.
- **Equal for hardlinks, and only for hardlinks.** Two names for one file must
  report one `qid.path`; that is the only signal the guest has.
- **Survives rename.** Renaming must not change it.
- **Do not derive it from a hash of the path.** Path-derived ids break under
  rename and collapse hardlinks into separate inodes.

`OBSERVED` — QEMU's server gets this right, and the spike can see it: a
hardlink pair in the lower layer arrives in the guest as a single inode.

    INFO   before copy-up: inode a=5485281 b=5485281 nlink=2

Two names, one inode number, `nlink=2` — that only happens because the server
returned the same `qid.path` for both. A server that allocated per-name ids
would show two different inodes here and `nlink=1`, and the guest would have no
way to know the files were related. This line is the single best regression
check for M3.

`qid.version` should change when contents change and `qid.type` must match the
file type (`QTFILE`/`QTDIR`/`QTSYMLINK`).

### B2. Do not expect `index=on` or `xino` to work over 9p — ACCEPT THE LIMIT
`SOURCE` `fs/overlayfs/super.c:395-417` calls `ovl_can_decode_fh()`; that
returns 0 unless the lower superblock exposes `s_export_op`
(`fs/overlayfs/util.c:79-90`).
`SOURCE` `fs/9p/` defines **no** `export_operations` anywhere in the tree.

Therefore, with a 9p lower, overlayfs unconditionally falls back to
`index=off, nfs_export=off`, and `xino=auto` degrades to `xino=off`.
`OBSERVED` — the spike's own dmesg, with the mount options that resulted:

    overlayfs: fs on '/mnt/lower' does not support file handles, falling back to index=off,nfs_export=off.
    overlayfs: fs on '/mnt/lower' does not support file handles, falling back to xino=off.
    root mount: overlay ... index=off,uuid=on,xino=off

This is a property of the **kernel's 9p client**, not of QEMU's server. Our own
9p server cannot fix it. See `RESULTS.md` for the behavioral consequence
(lower-layer hardlinks break on copy-up); it is a design constraint to absorb,
not a bug to chase at M3.

---

## C. Metadata fidelity

### C1. `TGETATTR` must return real uid/gid — REQUIRED
`OBSERVED` — the spike asserts a lower file owned `1234:5678` reads back as
`1234:5678` through the overlay, and still does after copy-up.
`SOURCE` `fs/overlayfs/copy_up.c:404-410` — copy-up explicitly re-applies
`ATTR_UID | ATTR_GID` on the new upper file from the lower's `stat`. Garbage in
from `TGETATTR` is garbage written durably into the upper layer.

Serve `st_uid`, `st_gid`, `st_mode`, `st_nlink`, `st_size`, `st_rdev`, and the
three timestamps. Set `valid` to exactly the fields actually filled.

### C2. Mode bits, including setuid/setgid, must round-trip — REQUIRED
`OBSERVED` — a `04755` file in the lower reads back as `4755` through the
overlay *and* keeps `4755` after copy-up. The spike asserts both halves
separately: a check that only compares before-vs-after passes on a server that
never delivered the bit at all. That weakness was found by mutation testing,
not by inspection — see the `setuid-mode` row in `RESULTS.md`.
`SOURCE` `fs/overlayfs/copy_up.c:398-402` applies `ATTR_MODE` from the lower
stat. A server masking off setuid silently downgrades guest binaries.

### C3. `st_nlink` must be truthful — REQUIRED
`OBSERVED` — the spike reads `nlink` on a lower hardlink pair. Combined with
B1, this is how the guest sees a hardlink at all.

### C4. Serve device nodes and symlinks — REQUIRED
`OBSERVED` for symlinks (`TREADLINK` through the overlay).
`INFERRED` for device nodes: container images contain them, and overlayfs
records its own whiteouts as char 0:0. `TMKNOD` and correct `st_rdev` in
`TGETATTR` are needed for an image lower layer to be faithful. M3 should cover
this explicitly; the spike's lower tree did not include a device node.

---

## D. Extended attributes

### D1. `TXATTRWALK` / `TXATTRCREATE` must work for `user.*` — REQUIRED
`OBSERVED` — the spike sets `user.spike` on a lower file at fixture time and
reads it back through the overlay.

### D2. Listing xattrs must work, not just getting one — REQUIRED
`SOURCE` `fs/overlayfs/copy_up.c:75-164` — `ovl_copy_xattr()` starts with a
`vfs_listxattr()` and iterates. A `TXATTRWALK` with an empty name string is the
list operation; returning an error there aborts the copy-up path early.

### D3. `EOPNOTSUPP` is tolerated for unknown xattrs, fatal for ACL and `security.*`
`SOURCE` `fs/overlayfs/copy_up.c:155-163`

    if (error != -EOPNOTSUPP || ovl_must_copy_xattr(name))
            break;
    /* Ignore failure to copy unknown xattrs */

and `ovl_must_copy_xattr()` (`copy_up.c:39-44`) is
`system.posix_acl_access`, `system.posix_acl_default`, and anything under
`security.`. So: a server may reject an exotic `user.*` xattr and copy-up still
succeeds, but rejecting a POSIX ACL or a `security.*` xattr **fails the whole
copy-up**. If the served tree carries ACLs or SELinux labels, they must be
retrievable.

### D4. `trusted.*` is an upper-layer concern, not a lower one — NOT REQUIRED
`SOURCE` `fs/overlayfs/dir.c:1062-1064` — `trusted.overlay.redirect` and the
other `trusted.overlay.*` bookkeeping xattrs are written to the **upper**
layer. Our lower is read-only, so the 9p server never needs to store them. It
must, however, not *invent* `trusted.overlay.*` entries on lower files, which
would be interpreted as overlay metadata.

---

## E. Behavior under the overlay

### E1. The lower must be genuinely read-only — REQUIRED
`OBSERVED` — the spike's negative control writes directly to the lower mount
and requires it to fail. Serve the lower with the read-only flag set; if writes
succeed, copy-up is not being exercised and every other result is meaningless.

### E2. `TREADDIR` offsets must be stable enough to resume — REQUIRED
`SOURCE` `fs/9p/vfs_dir.c:191` — the client stores `curdirent.d_off` as
`ctx->pos` and resumes from it. Offsets must be usable as opaque resume
cookies across separate `TREADDIR` calls on the same fid. A server that
renumbers entries between calls will drop or duplicate entries in a large
directory.

### E3. `TWALK` on a path with no components must clone the fid — REQUIRED
`INFERRED` from general client behavior: the client clones fids constantly to
keep a stable handle while walking. A server that treats a zero-component walk
as an error will fail in confusing, unrelated-looking ways.

### E4. Caching mode changes the contract — DECIDE AND PIN
`OBSERVED` — the spike ran `cache=loose`. Under `cache=loose` the client
trusts its page cache and does not revalidate, which is correct only because
our lower layer is immutable for the lifetime of a mount. If the JS side can
mutate a layer under a running guest, `cache=loose` will serve stale data and
the mode must change (at a substantial metadata-rate cost). Treat "lower layers
are immutable while mounted" as an invariant the JS side must uphold.

---

## F. What M3 must test that this spike could not

The spike used QEMU's 9p server, so it settles only that *a correct* 9p lower
works. These stay open for our own server:

1. Every item above marked `INFERRED` — `msize` negotiation (A3), device nodes
   (C4), zero-component walk (E3).
2. `qid.path` stability under our own allocator, specifically across rename,
   remount, and layer-cache eviction (B1). This is the highest-risk item: it is
   invisible until it corrupts something.
3. Hardlink detection across a merged multi-layer tree, where two names may
   arrive from different source layers.
4. Behavior when the JS side cannot answer — an evicted layer chunk, a network
   stall. 9p has no "try again later"; the server must block or return a real
   errno, never a short read.
5. Metadata throughput: the plan's own M3 gate wants >= 2,000 ops/s, and every
   overlayfs lookup is several 9p round trips.
