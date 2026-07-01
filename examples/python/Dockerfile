# Blank-slate Python image — installs ONLY the packaged snapdir wheel.
# Proves a real end user can consume the wheel from a bare python:3.12-slim base.
# Zero build tools: the wheel embeds a prebuilt .abi3.so native extension.

FROM python:3.12-slim

WORKDIR /app

# Install the wheel (prebuilt .abi3.so; no compiler or Rust toolchain needed).
# The whl is staged by run_relay.sh from the proven python-pack recipe output.
# Keep the original filename (pip enforces the wheel naming convention).
COPY snapdir-*.whl ./
RUN WHEEL=$(ls snapdir-*.whl) && pip install --no-cache-dir "./${WHEEL}"

# Copy the canonical example app.
COPY app.py ./

ENTRYPOINT ["python3", "app.py"]
