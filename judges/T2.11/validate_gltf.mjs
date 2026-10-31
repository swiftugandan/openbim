// Khronos glTF-Validator over one .gltf file; exit 0 iff zero errors. Usage: node validate_gltf.mjs f.gltf
import { readFile } from "node:fs/promises";
import { oracle } from "../lib/oracle.mjs";

const file = process.argv[2];
const v = await oracle("gltf-validator");
const bytes = new Uint8Array(await readFile(file));
const report = await v.validateBytes(bytes, { uri: file, maxIssues: 50 });
const errs = report.issues.messages.filter((m) => m.severity === 0);
if (report.issues.numErrors !== 0) {
  console.log(`glTF-Validator: ${report.issues.numErrors} errors: ` +
    errs.slice(0, 5).map((m) => `${m.code} ${m.pointer ?? ""} ${m.message}`).join("; "));
  process.exit(1);
}
console.log(`glTF-Validator: 0 errors, ${report.issues.numWarnings} warnings`);
