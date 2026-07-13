#!/usr/bin/env bash
set -euo pipefail
# Main sources are already compiled to build/classes and the .so is already
# copied to src/main/resources/native/... by the caller (verification command).
# This script compiles the test sources and runs both test classes.

# Compile test sources against the already-compiled main classes.
# -encoding UTF-8: the test files contain UTF-8 Unicode characters.
javac --release 17 --add-modules jdk.incubator.foreign \
    -encoding UTF-8 \
    -cp build/classes \
    -d build/classes \
    $(find src/test/java -name '*.java')

# Run regression guard first, then the new SizeTest.
java --add-modules jdk.incubator.foreign \
    --enable-native-access=ALL-UNNAMED \
    -cp build/classes:src/main/resources \
    io.snapdir.SnapdirApiTest

java --add-modules jdk.incubator.foreign \
    --enable-native-access=ALL-UNNAMED \
    -cp build/classes:src/main/resources \
    io.snapdir.SizeTest
