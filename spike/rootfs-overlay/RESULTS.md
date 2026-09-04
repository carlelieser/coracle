# M0 rootfs spike — results

**Verdict: the fixed decision in `docs/plan.md` section 2 holds.** A mainline
arm64 kernel boots with `overlayfs(lowerdir=9p, upperdir=ext4-on-virtio-blk)`
as its root filesystem, and copy-up, whiteouts, rename (including across the
overlay boundary), xattrs, uid/gid, and setuid bits all behave correctly. One
real limitation was found and is characterised below: **hardlinks that exist in
the lower layer are broken by copy-up**. That follows from the kernel's 9p
client exposing no `export_op`, so no 9p server implementation — QEMU's or
ours — can change it.

The JS-side union fallback is not needed.

## Scope

This uses **QEMU's own 9p server**. It settles the kernel-side question only:
that overlayfs accepts a correct 9p lower layer and behaves properly on it. It
says nothing about whether our future JS 9p server has the required fidelity —
that is M3's gate, and `9p-checklist.md` is the list it must satisfy.

## Versions pinned

| Component | Version | Note |
|---|---|---|
| Kernel | **6.12.107** | mainline, `cdn.kernel.org`, sha256 `a5f8c5be…07a`, built from source |
| QEMU | **10.0.11** | Debian trixie `qemu-system-arm`, running **inside** a `linux/arm64` container |
| busybox | **1.37.0** | Alpine 3.21 `busybox-static` prebuilt |
| Machine | `virt`, `-cpu cortex-a72`, 1 vCPU, 1 GB | virtio-**mmio** transport, matching the plan's machine model |
| Host | macOS arm64 (Darwin 25.5.0) | guests run natively, no cross-arch emulation |

Exact values live in `versions.env`; the run fails on a checksum mismatch.

Note the host QEMU (brew, 11.1.1) is **not** what runs the spike. See
"QEMU runs in a container" below — that is a load-bearing detail, not a
convenience.

## Reproducing

```
./run-spike.sh        # builds if needed, boots, prints a per-behavior verdict
./verify-harness.sh   # breaks the setup on purpose; proves the tests can fail
```

First run builds the kernel (15-40 min, cached afterwards); the kernel and
busybox are cached separately, so a missing busybox does not trigger a kernel
rebuild. `run-spike.sh` exits non-zero unless every case passes.

Artifacts land in `build/`: `boot.log` is the guest console (the evidence
quoted below), `run.log` is the full container transcript including the
pre-boot self-check, `kernel.config` is the exact config built.

## Results

From `build/boot.log`, kernel 6.12.107 under QEMU 10.0.11. Overlay root
confirmed before any test ran:

```
stage2: / is fstype=overlay
SPIKE_BOOT: OK root=overlay
INFO   root mount: overlay overlay rw,relatime,lowerdir=/mnt/lower,
       upperdir=/mnt/upper/upper,workdir=/mnt/upper/work,index=off,uuid=on,xino=off
```

`SPIKE_SUMMARY pass=15 fail=0 skip=0 known=1`

| Behavior | Verdict | Evidence |
|---|---|---|
| Boot to overlay root | PASS | `/` reports `fstype=overlay`; the test aborts if it does not |
| readdir d_type | PASS | `9p lower /fixtures: dirs=3 files=7 symlinks=1` |
| Copy-up | PASS | see below |
| Whiteout | PASS | see below |
| Opaque directory | PASS | `merged subdir after recreate: 'fresh.txt '` — the two lower entries stayed hidden |
| Rename (upper) | PASS | `renamed content is 'rename-payload' as expected` |
| Rename across overlay boundary | PASS | see below |
| Rename directory across boundary | PASS | `redirect xattr present on moved dir: 1`; contents intact |
| xattr | PASS | see below |
| uid/gid | PASS | see below |
| setuid bit | PASS | `setuid file mode: before=4755 after copy-up=4755` |
| Hardlink (upper) | PASS | `inodes a=32797 b=32797 nlink=2`; write through a visible via b |
| **Hardlink (lower, across copy-up)** | **KNOWN LIMITATION** | see below |
| Symlink through 9p lower | PASS | `target='hello.txt'` resolves to the copied-up content |
| Persistence to ext4 upper | PASS | marker written through overlay found on the ext4 upper |
| Negative control | PASS | `touch` on the lower: `Read-only file system` |

### Copy-up

```
INFO   lower content before: 'lower-original-content'
INFO   upper before copy-up: ... No such file or directory / absent (expected)
INFO   merged content after: 'lower-original-content|appended-by-guest|'
INFO   lower content after:  'lower-original-content'
```

The file materialised in the upper layer, the original content survived, the
append is visible, and the lower layer was not touched.

### Whiteout

```
INFO   upper whiteout marker: c---------    2 0        0           0,   0 .../fixtures/deleteme.txt
INFO   still in lower: yes / hidden in merged: yes
INFO   occurrences in merged readdir: 0
```

The whiteout is a char device 0:0 in the upper, as overlayfs specifies. The
test checks readdir as well as `stat` — a file hidden from one but not the
other is a real bug class and would be caught.

### Rename across the overlay boundary

```
INFO   lower-only file content: 'movable-content'
PASS rename-cross-layer: lower-only file copied up and moved, lower intact, content preserved
```

No `EXDEV`. Directory rename also succeeded, with
`trusted.overlay.redirect` set on the result — so `redirect_dir` is doing its
job over a 9p lower.

### xattrs

```
INFO   xattr read from lower via merged view: 'lower-xattr-value'
INFO   xattr set rc=0 err='' readback='testvalue'
```

Both directions work: an xattr set on the lower at fixture time is readable
through the overlay, and a new xattr can be set on the upper.

### uid/gid and mode

```
INFO   lower-only file ownership through overlay: 1234:5678
INFO   after copy-up, ownership through overlay: 1234:5678 (upper file: 1234:5678)
INFO   setuid file mode: before=4755 after copy-up=4755 (want 4755 both)
```

Ownership survives 9p transport *and* copy-up, and the setuid bit is not
dropped. This is the case most likely to break silently, so it is asserted on
the merged view and on the upper file independently, and the setuid check
asserts the absolute value at both points rather than only that it did not
change — see the mutation-testing note below for why that distinction matters.

## The one real limitation: lower-layer hardlinks break on copy-up

```
INFO   before copy-up: inode a=5485281 b=5485281 nlink=2
INFO   after copy-up of a: inode a=32798 b=5485281
INFO   b content after writing through a: 'linked-content|'
```

Two names for one file in the lower layer are correctly presented as one inode
(QEMU's 9p server gives stable QIDs). After writing through `link-a.txt`, only
that name is copied up; `link-b.txt` still resolves to the lower file and does
**not** see the write. The link is severed.

This is expected, and the kernel says so out loud:

```
overlayfs: fs on '/mnt/lower' does not support file handles,
           falling back to index=off,nfs_export=off.
```

Overlayfs needs `index=on` to rejoin a copied-up hardlink, and `index` requires
the lower filesystem to encode file handles. Reading the source:
`ovl_can_decode_fh()` (`fs/overlayfs/util.c:79`) returns 0 unless the lower
superblock exposes `s_export_op`, and **`fs/9p/` defines no export operations
anywhere in the tree**. So this is a property of the kernel's 9p *client*, not
of QEMU's server: **our own 9p server cannot fix it either.**

Impact on the plan: a container image whose layers use hardlinks (busybox and
toybox images do this heavily for applet names) will see links silently split
the first time one of them is written. Reads are unaffected, and each name keeps
correct content until written. This is the same behavior Docker's own
`overlay2` driver has, so images in practice tolerate it. Worth an explicit
note in M3/M4, not a redesign. The JS-side union fallback would have to solve
this itself, so the fallback is not obviously better here.

## What this changes in the plan

Nothing in section 2 flips. Three things are worth folding in:

1. **M3's 9p gate should cite `9p-checklist.md` item by item**, and the
   `qid.path` requirement (B1) deserves a dedicated test — it is the item most
   likely to be implemented plausibly and wrongly, and the failure is invisible
   until it corrupts something.
2. **The lower-layer hardlink split should be written down as accepted
   behavior**, not discovered again at M4 when a busybox image behaves oddly.
3. **`index=off` and `xino=off` are permanent** with a 9p lower. Anything later
   that assumes stable `st_ino` across the overlay — a guest process caching
   inode numbers, or `nfs_export` — will not get it.

## Kernel config required

Everything must be built in (`=y`), not modular: the root filesystem is
assembled by the initramfs before any module could be loaded from it.

```
CONFIG_OVERLAY_FS=y
CONFIG_OVERLAY_FS_REDIRECT_DIR=y      # cross-layer directory rename
CONFIG_OVERLAY_FS_INDEX=y             # compiled in; still forced off at runtime over 9p
CONFIG_OVERLAY_FS_XINO_AUTO=y         # likewise degrades to xino=off
CONFIG_OVERLAY_FS_METACOPY=y
CONFIG_NET_9P=y
CONFIG_NET_9P_VIRTIO=y
CONFIG_9P_FS=y
CONFIG_9P_FS_POSIX_ACL=y
CONFIG_9P_FS_SECURITY=y
CONFIG_EXT4_FS=y
CONFIG_EXT4_FS_POSIX_ACL=y
CONFIG_EXT4_FS_SECURITY=y
CONFIG_VIRTIO=y
CONFIG_VIRTIO_MMIO=y                  # mmio, not PCI, per the plan's machine model
CONFIG_VIRTIO_BLK=y
CONFIG_BLK_DEV_INITRD=y
CONFIG_DEVTMPFS=y / CONFIG_DEVTMPFS_MOUNT=y
CONFIG_SERIAL_AMBA_PL011=y / _CONSOLE=y
```

`build-guest.sh` verifies every one of these actually landed in `.config`
after `olddefconfig` and refuses to build otherwise — `merge_config.sh` can
silently drop an option whose dependencies are unmet, which would produce a
kernel that fails the spike for an invisible reason.

Mount line used:

```
mount -t 9p -o trans=virtio,version=9p2000.L,msize=512000,cache=loose,access=client lower /mnt/lower
mount -t overlay overlay -o lowerdir=/mnt/lower,upperdir=/mnt/upper/upper,workdir=/mnt/upper/work /mnt/root
```

## Quirks and workarounds

**QEMU runs inside a linux/arm64 container, not on the macOS host.** This is
the single most important detail for anyone reproducing the spike. The lower
tree served over 9p must carry real Linux ownership and `user.*` xattrs. On a
Docker bind mount to macOS neither survives, and the failure is silent in the
worst way:

```
chown ok                                    <- reports success
setfattr: /p/f.txt: Operation not supported
/p/f.txt 0:0 nlink=2                        <- but the ownership did not take
```

A spike built that way would have failed the uid/gid and xattr cases for a host
reason and been misread as an overlayfs-over-9p limitation.
`in-container-run.sh` builds the lower tree on the container's own filesystem
and, before booting, **probes a scratch file** to confirm the filesystem can
carry ownership, `user.*` xattrs, and hardlinks at all — aborting if not. The
probe deliberately uses its own scratch file rather than a fixture: asserting a
fixture's value there would pre-empt the guest-side test and hide whether that
test actually works.

**`switch_root`, not `pivot_root`.** The initramfs root is rootfs, which the
kernel refuses to pivot away from; `pivot_root` returns `EINVAL`. The lower
layer must therefore carry `/bin/sh`, because `switch_root` discards the
initramfs — the spike puts busybox in the lower layer, which is what a real
image layer does anyway.

**`security_model=passthrough`** on the QEMU fsdev, so the server reports the
real uid/gid rather than mapping everything to the QEMU process owner.

**busybox is not built from source.** 1.37.0 does not compile against GCC 12 on
aarch64 (`libbb/hash_md5_sha.c`, undeclared `sha1_process_block64`). Alpine's
patched static build of the same version is used instead. The guest shell is
not what this spike measures.

## Evidence quality

Three things back the verdict beyond the passing table:

1. **A negative control inside the run.** The lower layer is mounted read-only
   and the matrix requires a direct write to it to fail. If it ever succeeds,
   copy-up is not being exercised and every other result is void.
2. **Preconditions are asserted, not assumed.** Missing layers or fixtures
   report `FAIL`/`SKIP`, never `PASS`.
3. **The harness is verified by mutation, and the mutation testing found a
   real bug in this suite.** `verify-harness.sh` breaks the setup in four
   specific ways and requires the matrix to go red each time:

   | Mutation | Detected by |
   |---|---|
   | lower xattr set under a different name | `xattr` |
   | fixture chowned to the wrong uid/gid | `uid-gid` |
   | 9p lower served read-write | `negative-control` |
   | setuid bit not set on the fixture | `setuid-mode` |

   The fourth mutation initially went **undetected**. The mode check was
   folded into the uid/gid case and only asserted `before == after` across
   copy-up — which passes trivially on a layer where the setuid bit was never
   present, and would therefore also pass on a 9p server that silently
   stripped it. That is precisely the "test passed because it did nothing"
   failure this spike is supposed to rule out. It is now a separate
   `setuid-mode` case asserting both halves: that 9p delivered `4755`, and
   that copy-up preserved it.

   With that fixed, all four mutations are detected — full output in
   `harness-verification.log`:

   ```
   DETECTED: 'xattr-wrong-name' correctly failed test 'xattr'
   DETECTED: 'wrong-owner'      correctly failed test 'uid-gid'
   DETECTED: 'writable-lower'   correctly failed test 'negative-control'
   DETECTED: 'no-setuid'        correctly failed test 'setuid-mode'
   harness verification PASSED: every mutation was detected
   ```

   The metadata PASSes in the table above should be read in that light: they
   are claims that have survived a deliberate attempt to make them lie.

Weaker spots, stated plainly:

- **Device nodes are untested.** The lower fixture tree has no `mknod` entry.
  Container images contain them; M3 must cover this.
- **Single-layer lower only.** The real system merges many image layers into
  one lower tree. Multi-layer merge behavior is not exercised here.
- **`cache=loose` assumes an immutable lower.** Correct for our design, but the
  spike does not test what happens if a layer changes under a running guest.
- **QEMU's 9p server is a well-behaved reference.** Every `OBSERVED` item in
  the checklist means "the kernel is satisfied by a correct server", not "any
  server will do".
