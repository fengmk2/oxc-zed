// Zed's Command has no cwd field. Keep Vite+ anchored to the declaring package,
// even when the editor opens a subdirectory or uses a global vp installation.
const { spawn } = require("node:child_process");
const path = require("node:path");
const [root, executable, loader, tool] = process.argv.slice(1);
const hint = "Vite+ language server failed. Install or upgrade vite-plus, then restart the language server.";
const pathKey = Object.keys(process.env).find((key) => key.toUpperCase() === "PATH") || "PATH";
const searchPath = process.env[pathKey] || "";
delete process.env[pathKey];
process.env.PATH = path.dirname(process.execPath) + path.delimiter + searchPath;
const child = spawn(loader === "node" ? process.execPath : executable,
  [...(loader === "node" ? [executable] : []), tool, "--lsp"],
  { cwd: root, env: process.env, stdio: "inherit" });
child.on("error", (error) => {
  console.error(`${hint}\n${error.message}`);
  process.exitCode = 1;
});
child.on("exit", (code, signal) => {
  if (code) console.error(hint);
  process.exitCode = code ?? (signal ? 1 : 0);
});
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}
process.on("exit", () => { if (child.exitCode === null) child.kill(); });
