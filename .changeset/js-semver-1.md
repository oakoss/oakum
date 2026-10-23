---
oakum: major
---

### Changed

oakum reads npm ranges with js-semver 1.0. A dependency range with `==` before a full version, such as `==1.0.0`, is now refused, as pnpm and node-semver refuse it; oakum read it as `1.0.0` before. `==` before a partial version, such as `==1`, still reads as before. The refusal for an unreadable range names what is wrong, such as `unexpected character '=' while parsing major version`, instead of `invalid number`.
