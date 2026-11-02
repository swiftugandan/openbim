#!/usr/bin/env bash
# One-time setup of the independent oracles the judges use, pinned, into .oracles/ (git-ignored),
# or into ORACLES_PREFIX when set (loop/cloud-env-setup.sh).
# Run once before loop/run.sh. Re-running is safe. Needs network (PyPI, npm, GitHub, rustup).
set -euo pipefail
cd "$(dirname "$0")/.."
P=${ORACLES_PREFIX:-.oracles}
mkdir -p "$P"

python_oracles() {
  echo "== Python oracles ($P/py, Python 3.12 via uv)"
  [ -x "$P/py/bin/python" ] || uv venv -q --python 3.12 "$P/py"
  uv pip install -q --python "$P/py/bin/python" \
    ifcopenshell==0.9.0.post1 ezdxf==1.4.5 pypdf==6.21.4 manifold3d==3.5.4 trimesh==5.2.0 \
    shapely==2.2.0 pynitefea==3.2.1 wntr==1.5.0 python-solvespace==3.0.8 xmlschema==4.3.2 \
    websockets==17.4 networkx==3.7 rtree==1.4.1 numpy==2.5.4 scipy==1.18.1
  "$P/py/bin/python" -c "import ifcopenshell, ezdxf, pypdf, manifold3d, trimesh, shapely, Pynite, wntr, python_solvespace, xmlschema, websockets, networkx"
}

node_oracles() {
  echo "== Node oracles ($P/node; Playwright drives the installed Google Chrome)"
  mkdir -p "$P/node"
  cat > "$P/node/package.json" <<'EOF'
{ "private": true, "type": "module", "dependencies": {
  "playwright-core": "1.64.1", "@axe-core/playwright": "4.14.0",
  "gltf-validator": "2.0.0-dev.3.10", "@modelcontextprotocol/sdk": "1.32.2" } }
EOF
  (cd "$P/node" && npm install --silent --no-audit --no-fund)
  [ -d "/Applications/Google Chrome.app" ] || command -v google-chrome >/dev/null \
    || echo "WARNING: Google Chrome not found; UI judges (T8.x) will fail"
}

bcf_schemas() {
  echo "== buildingSMART BCF 3.0 XSDs ($P/bcf-xsd, tag v3.0)"
  mkdir -p "$P/bcf-xsd"
  for f in documents extensions markup project shared-types version visinfo; do
    curl -sfL -o "$P/bcf-xsd/$f.xsd" "https://raw.githubusercontent.com/buildingSMART/BCF-XML/v3.0/Schemas/$f.xsd"
  done
}

rust_tools() {
  echo "== Rust: the toolchain and wasm targets pinned in rust-toolchain.toml, and wasm-pack"
  rustup toolchain install
  touch "$P/rust-wasip2"
  command -v wasm-pack >/dev/null && return 0
  if [ "$(uname -s)" = Linux ]; then
    curl -sfL "https://github.com/rustwasm/wasm-pack/releases/download/v0.12.1/wasm-pack-v0.12.1-$(uname -m)-unknown-linux-musl.tar.gz" \
      | tar -xz --strip-components=1 -C "$(dirname "$(command -v cargo)")" --wildcards '*/wasm-pack'
  else
    cargo install wasm-pack
  fi
}

# Independent installs run side by side: a cloud environment only caches a setup under about five minutes.
python_oracles & py=$!
node_oracles & node=$!
bcf_schemas & bcf=$!
rust_tools & rust=$!
wait $py; wait $node; wait $bcf; wait $rust

echo "== IFC reference corpus (corpus/ifc) — provided by you, not downloaded here"
if ls corpus/ifc/*.ifc >/dev/null 2>&1; then
  ls corpus/ifc/*.ifc
else
  echo "corpus/ifc/ is empty: T5.4 will be marked blocked until you add IFC4 files, e.g."
  echo "  Duplex_A / Duplex_MEP (buildingSMART sample files), Schependomlaan (github.com/openBIMstandards/DataSetSchependomlaan)"
fi
echo "setup complete"
