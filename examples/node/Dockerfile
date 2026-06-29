# Blank-slate Node.js image — installs ONLY the packaged @snapdir/snapdir artifact.
# Proves a real end user can consume the npm package from a bare node:22-slim base.
# Zero build tools: the tarball embeds a prebuilt native .node addon.

FROM node:22-slim

WORKDIR /app

# Install the packaged tarball (prebuilt .node binary; no compiler needed).
# The tgz is staged by run_relay.sh from the proven node-pack recipe output.
COPY snapdir.tgz ./
RUN npm install ./snapdir.tgz

# Copy the canonical example app.
COPY app.mjs ./

ENTRYPOINT ["node", "app.mjs"]
