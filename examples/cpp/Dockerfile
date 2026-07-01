# Blank-slate C++ image — installs ONLY the packaged snapdir C++ artifacts.
# gcc:13 has g++ -std=c++20 support; no other toolchain needed.
# The header-only wrapper (snapdir.hpp + snapdir.h) and the static lib
# (libsnapdir_ffi.a) are pre-built and staged by run_relay.sh.

FROM gcc:13

WORKDIR /app

# Copy the packaged C++ binding artifacts (staged by run_relay.sh).
COPY snapdir.hpp ./
COPY snapdir.h ./
COPY libsnapdir_ffi.a ./

# Copy the canonical example app and compile it.
COPY app.cpp ./
RUN g++ -std=c++20 -Wall -Wextra app.cpp -I. -L. -lsnapdir_ffi -lpthread -ldl -lm -o app

ENTRYPOINT ["/app/app"]
