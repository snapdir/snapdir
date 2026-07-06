# snapdir (Java binding)

Java bindings for [snapdir](https://snapdir.org) — content-addressed directory
snapshots. Uses the JDK Foreign Function API over the snapdir C ABI
(`libsnapdir_ffi`), which is embedded in the jar and extracted at runtime by
`NativeLoader`.

## Install (Maven Central)

```xml
<dependency>
  <groupId>org.snapdir</groupId>
  <artifactId>snapdir</artifactId>
  <version>1.11.0</version>
</dependency>
```

Gradle:

```kotlin
implementation("org.snapdir:snapdir:1.11.0")
```

Run with the incubator Foreign Function module enabled (JDK 17):

```sh
java --add-modules jdk.incubator.foreign --enable-native-access=ALL-UNNAMED ...
```

## Platform support

The 1.11.0 jar embeds **glibc**-linked native libraries for
`linux-x86_64`, `linux-aarch64`, `mac-x86_64`, and `mac-aarch64`. It runs on
standard **glibc** JVMs (e.g. `eclipse-temurin:17-jdk`).

> **Alpine / musl is not yet supported.** The embedded native library is
> glibc-linked and `NativeLoader` has no musl detection, so the jar cannot load
> on a musl JVM (`UnsatisfiedLinkError`). Alpine/musl support is planned for
> 1.11.1 (musl `java-native` build legs + a libc probe in `NativeLoader`). Every
> other snapdir binding (npm, PyPI, crates.io, cpp, zig, go) runs on Alpine musl
> today.

Verified end-to-end from Maven Central by `.github/workflows/verify-published.yml`
(the `java` job installs this coordinate on `eclipse-temurin:17-jdk` and runs the
canonical example, asserting the golden snapshot id).
