# GPL-3.0 Compatible Licenses Guide

This document explains which licenses are compatible with your GPL-3.0-or-later project.

## Your Current Configuration

Your `deny.toml` is configured to:
- ✅ Only include `GPL-3.0-or-later` in the active allow list (no warnings!)
- 📝 Keep all other GPL-compatible licenses in commented form
- 🔧 Easy to uncomment licenses as you add dependencies that need them

This approach eliminates the `warning[license-not-encountered]` warnings while keeping all compatible licenses documented and ready to use.

## ✅ Compatible Licenses (Allowed)

### Most Common in Rust Ecosystem

These are the licenses you'll encounter most frequently in Rust crates:

1. **MIT** - ~45% of Rust crates
   - Very permissive, allows almost anything
   - GPL-compatible ✅
   
2. **Apache-2.0** - ~40% of Rust crates
   - Permissive with patent protection
   - GPL-compatible ✅
   
3. **MIT OR Apache-2.0** - Dual licensed (most common pattern)
   - You can choose either license
   - GPL-compatible ✅

### BSD Family

4. **BSD-2-Clause** - Simplified BSD
5. **BSD-3-Clause** - Original BSD (modified)
6. **0BSD** - Public domain-like BSD
7. **ISC** - Similar to MIT

All GPL-compatible ✅

### GPL Family

8. **GPL-3.0-only** - Exact same version as yours
9. **GPL-3.0-or-later** - Your project's license
10. **LGPL-3.0** - Lesser GPL (library version)
11. **LGPL-2.1** - Older Lesser GPL (compatible with GPL-3.0)
12. **AGPL-3.0** - Affero GPL (network-protective variant)

All GPL-compatible ✅

### Public Domain / Highly Permissive

13. **CC0-1.0** - Creative Commons public domain dedication
14. **Unlicense** - Public domain dedication
15. **BSL-1.0** - Boost Software License
16. **Zlib** - zlib/libpng license

All GPL-compatible ✅

### Other Compatible

17. **MPL-2.0** - Mozilla Public License (file-level copyleft)
18. **Unicode-DFS-2016** - Unicode data files license

Both GPL-compatible ✅

## ❌ Incompatible Licenses (NOT Allowed)

### NEVER Use These With GPL-3.0

1. **GPL-2.0-only** (without "or later") ❌
   - **Major incompatibility!** GPL-2.0-only and GPL-3.0 cannot be mixed
   - FSF considers them incompatible
   - Will cause legal issues

2. **Apache-1.0** / **Apache-1.1** ❌
   - Older Apache licenses are GPL-incompatible
   - Apache-2.0 is fine, but 1.x is not

3. **BSD-4-Clause** ❌
   - Original BSD with advertising clause
   - Incompatible with GPL due to additional restrictions

4. **OpenSSL** ❌
   - Custom license incompatible with GPL
   - Use `rustls` instead for TLS in GPL projects

5. **Proprietary licenses** ❌
   - Obviously incompatible

## Special Cases

### Dual Licensed Crates

Many Rust crates use dual licensing:
```toml
license = "MIT OR Apache-2.0"
```

✅ **This is compatible!** You can use the dependency under either license terms (choose MIT or Apache-2.0).

### GPL-2.0-or-later

```toml
license = "GPL-2.0-or-later"
```

✅ **This IS compatible** because "or-later" allows using it under GPL-3.0 terms.

### LGPL (Lesser GPL)

LGPL is designed for libraries and is compatible with GPL:
- You can link LGPL libraries into your GPL program
- LGPL allows proprietary software to use it (via dynamic linking)
- GPL requires the entire work to be GPL

## What cargo-deny Does

When you run `cargo deny check licenses`, it will:

1. ✅ **Allow** dependencies with licenses in your `allow` list
2. ❌ **Reject** dependencies with licenses NOT in your `allow` list
3. ⚠️ **Warn** about licenses in the allow list that weren't encountered (expected if you have no dependencies yet)

## When You Add Dependencies

### If a Dependency is Rejected

```bash
error[rejected]: failed to satisfy license requirements
  ┌─ Cargo.toml:10:1
  │
10│ some-crate = "1.0"
  │ license MIT is not explicitly allowed
```

**Steps:**
1. Check if the license is GPL-compatible (see list above or the commented section in `deny.toml`)
2. If compatible, **uncomment the license** in `deny.toml`'s allow list:
   ```toml
   allow = [
       "GPL-3.0-or-later",
       "MIT",  # <-- Uncomment this line
   ]
   ```
3. If NOT compatible, find an alternative crate

### Quick Fix Examples

**For MIT-licensed dependency:**
```toml
# In deny.toml, change this:
allow = [
    "GPL-3.0-or-later",
]

# To this:
allow = [
    "GPL-3.0-or-later",
    "MIT",
]
```

**For Apache-2.0-licensed dependency:**
```toml
allow = [
    "GPL-3.0-or-later",
    "Apache-2.0",
]
```

**For dual-licensed MIT/Apache-2.0 dependency (most common):**
```toml
allow = [
    "GPL-3.0-or-later",
    "MIT",
    "Apache-2.0",
]
```

### Common GPL-Incompatible Crates to Avoid

Some popular crates you may need alternatives for:

❌ **openssl** → ✅ Use **rustls** instead  
❌ Crates with OpenSSL dependency → ✅ Look for `*-rustls` variants (e.g., `reqwest` with `rustls` feature)

## Verifying License Compatibility

To check if a license is GPL-compatible:

1. **FSF's List**: https://www.gnu.org/licenses/license-list.html
2. **SPDX Database**: https://spdx.org/licenses/
3. **Ask the FSF**: licensing@fsf.org

## Testing Your Configuration

```bash
# Check all license compliance
cargo deny check licenses

# Full check (advisories, bans, licenses, sources)
cargo deny check

# Using Makefile
make audit
```

## Summary

✅ **Your configuration allows all common GPL-compatible licenses**  
✅ **Most Rust crates use MIT and/or Apache-2.0 (both compatible)**  
✅ **You're protected from accidentally using incompatible licenses**  
⚠️ **Watch out for GPL-2.0-only and OpenSSL**

## References

- [GNU GPL Compatibility](https://www.gnu.org/licenses/license-list.html)
- [SPDX License List](https://spdx.org/licenses/)
- [FSF Licensing Guide](https://www.fsf.org/licensing/)
- [Rust License Practices](https://rust-lang.github.io/api-guidelines/necessities.html#crate-and-its-dependencies-have-a-permissive-license-c-permissive)

