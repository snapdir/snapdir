# bindings/cpp/Makefile — build + run the snapdir C++ binding tests.
#
# The snapdir-bindings image has no cmake; we drive the native (arm64) clang++
# and g++ directly — both already in the image and both must be -Werror clean.
# The ffi staticlib (lib/libsnapdir_ffi.a) and the C ABI header
# (include/snapdir.h) are vendored at build time (gitignored) by
# `cargo build --release -p snapdir-ffi` before this Makefile runs.

CXX      ?= clang++
CXXSTD   ?= -std=c++20
WARN     := -Wall -Wextra -Werror
INCLUDES := -Iinclude
LDFLAGS  := -Llib -lsnapdir_ffi -lpthread -ldl -lm
BUILDDIR := build

TEST_SRC := test/cpp_api.cpp
TEST_BIN := $(BUILDDIR)/cpp_api_test

# ── install / packaging (dry-run; the C/C++ binding has no registry) ──────────
# Lays down the public header(s), the static lib, and a pkg-config file under a
# prefix. The C ABI header (include/snapdir.h) and lib/libsnapdir_ffi.a are
# vendored at build time by `cargo build --release -p snapdir-ffi`.
PREFIX  ?= /usr/local
VERSION ?= 0.1.0
INCDIR  := $(PREFIX)/include
LIBDIR  := $(PREFIX)/lib
PCDIR   := $(LIBDIR)/pkgconfig

.PHONY: build test gcc-syntax clean install

build: $(TEST_BIN)

$(TEST_BIN): $(TEST_SRC) include/snapdir.hpp include/snapdir.h lib/libsnapdir_ffi.a
	@mkdir -p $(BUILDDIR)
	$(CXX) $(CXXSTD) $(WARN) $(INCLUDES) $(TEST_SRC) -o $@ $(LDFLAGS)

# Compile + run the black-box contract test.
test: build
	$(TEST_BIN)

# Second-compiler cleanliness gate (cpp-quality also runs this): the header +
# test must be -Werror clean under g++ as well as clang++.
gcc-syntax:
	g++ $(CXXSTD) $(WARN) $(INCLUDES) -fsyntax-only $(TEST_SRC)

clean:
	rm -rf $(BUILDDIR)

# Dry-run install: header-only RAII wrapper + vendored C header + static lib +
# a pkg-config file (prefix/version substituted into snapdir.pc.in).
install:
	@mkdir -p $(INCDIR) $(LIBDIR) $(PCDIR)
	install -m 0644 include/snapdir.hpp $(INCDIR)/snapdir.hpp
	install -m 0644 include/snapdir.h   $(INCDIR)/snapdir.h
	install -m 0644 lib/libsnapdir_ffi.a $(LIBDIR)/libsnapdir_ffi.a
	sed -e 's|@PREFIX@|$(PREFIX)|g' -e 's|@VERSION@|$(VERSION)|g' \
	    snapdir.pc.in > $(PCDIR)/snapdir.pc
