#!/usr/bin/env bash
# One-time setup of the independent oracles the judges use, pinned, into .oracles/ (git-ignored).
# Run once before loop/run.sh. Re-running is safe. Needs network (PyPI, npm, GitHub, rustup).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .oracles

echo "== Python oracles (.oracles/py, Python 3.12 via uv)"
[ -x .oracles/py/bin/python ] || uv venv -q --python 3.12 .oracles/py
uv pip install -q --python .oracles/py/bin/python \
  ifcopenshell==0.9.0.post1 ezdxf==1.4.5 pypdf==6.21.4 manifold3d==3.5.4 trimesh==5.2.0 \
  shapely==2.2.0 pynitefea==3.2.1 wntr==1.5.0 python-solvespace==3.0.8 xmlschema==4.3.2 \
  websockets==17.4 networkx==3.7 rtree==1.4.1 numpy==2.5.4 scipy==1.18.1
.oracles/py/bin/python -c "import ifcopenshell, ezdxf, pypdf, manifold3d, trimesh, shapely, Pynite, wntr, python_solvespace, xmlschema, websockets, networkx"

echo "== Node oracles (.oracles/node; Playwright drives the installed Google Chrome)"
mkdir -p .oracles/node
cat > .oracles/node/package.json <<'EOF'
{ "private": true, "type": "module", "dependencies": {
  "playwright-core": "1.64.1", "@axe-core/playwright": "4.14.0",
  "gltf-validator": "2.0.0-dev.3.10", "@modelcontextprotocol/sdk": "1.32.2" } }
EOF
(cd .oracles/node && npm install --silent --no-audit --no-fund)
test -d "/Applications/Google Chrome.app" || echo "WARNING: Google Chrome not found; UI judges (T8.x) will fail"

echo "== buildingSMART BCF 3.0 XSDs (.oracles/bcf-xsd, tag v3.0)"
mkdir -p .oracles/bcf-xsd
for f in documents extensions markup project shared-types version visinfo; do
  curl -sfL -o ".oracles/bcf-xsd/$f.xsd" "https://raw.githubusercontent.com/buildingSMART/BCF-XML/v3.0/Schemas/$f.xsd"
done

echo "== Rust targets: wasm32-unknown-unknown (bim-wasm), wasm32-wasip2 (component plugins)"
rustup target add wasm32-unknown-unknown wasm32-wasip2
touch .oracles/rust-wasip2
command -v wasm-pack >/dev/null || cargo install wasm-pack

echo "== IFC reference corpus (corpus/ifc) — provided by you, not downloaded here"
if ls corpus/ifc/*.ifc >/dev/null 2>&1; then
  ls corpus/ifc/*.ifc
else
  echo "corpus/ifc/ is empty: T5.4 will be marked blocked until you add IFC4 files, e.g."
  echo "  Duplex_A / Duplex_MEP (buildingSMART sample files), Schependomlaan (github.com/openBIMstandards/DataSetSchependomlaan)"
fi
echo "setup complete"
