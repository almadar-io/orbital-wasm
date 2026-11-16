// Writes `<pkg>/orbital_wasm_instance.js` (+ `.d.ts`) beside the wasm-bindgen glue `<pkg>/orbital_wasm.js`.
// The glue keeps one WebAssembly instance per loaded module, so every kernel a host opens shares one memory, which only
// grows. The factory runs the same glue in a fresh scope per call: each call is an instance with its own memory, freed
// once nothing references it (G-ORBITAL-WASM-009). Usage: node scripts/instance-factory.mjs <pkg dir>
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const dir = process.argv[2];
if (dir === undefined) {
  console.error('usage: node scripts/instance-factory.mjs <pkg dir>');
  process.exit(2);
}
const glue = readFileSync(join(dir, 'orbital_wasm.js'), 'utf8');
const lines = glue.split('\n');
const names = [];
const body = lines.map((line) => {
  const declared = /^export (class|function) ([A-Za-z_$][\w$]*)/.exec(line);
  if (declared !== null) {
    names.push(declared[2]);
    return line.slice('export '.length);
  }
  if (/^export \{ initSync, __wbg_init as default \};$/.test(line)) return '';
  if (/^(export|import)\b/.test(line)) throw new Error(`instance-factory: the glue has a module-level line this factory does not know: ${line}`);
  return line;
});
for (const needed of ['__wbg_init', 'installLanguage', 'Kernel']) {
  if (!body.some((line) => line.startsWith(`function ${needed}(`) || line.startsWith(`async function ${needed}(`) || line.startsWith(`class ${needed} `))) {
    throw new Error(`instance-factory: the glue no longer defines ${needed}`);
  }
}

// Instantiated asynchronously: a browser refuses `new WebAssembly.Instance` on its main thread for a module over 8 MB.
writeFileSync(join(dir, 'orbital_wasm_instance.js'), `// Generated from orbital_wasm.js by scripts/instance-factory.mjs; do not edit.
/** A kernel instance with its own memory: \`module\` instantiated once more and \`languageBundle\` installed in it. */
export async function createKernelInstance(module, languageBundle) {
${body.join('\n')}
await __wbg_init({ module_or_path: module });
installLanguage(languageBundle);
return { memory: wasm.memory, ${names.join(', ')} };
}
`);

writeFileSync(join(dir, 'orbital_wasm_instance.d.ts'), `// Generated from orbital_wasm.d.ts by scripts/instance-factory.mjs; do not edit.
import type * as Glue from './orbital_wasm.js';

/** The glue's exports bound to one instance, and that instance's memory. */
export type KernelInstance = Omit<typeof Glue, 'default' | 'initSync'> & { readonly memory: WebAssembly.Memory };

/** A kernel instance with its own memory: \`module\` instantiated once more and \`languageBundle\` installed in it. */
export declare function createKernelInstance(module: WebAssembly.Module, languageBundle: string): Promise<KernelInstance>;
`);
console.log(`instance-factory: ${join(dir, 'orbital_wasm_instance.js')} (${names.length} exports)`);
