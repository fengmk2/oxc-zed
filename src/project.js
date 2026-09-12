// Filesystem access for the Rust resolver. WASI can only read the extension's
// directory; Zed's worktree API cannot read ancestors of an opened subpackage.
const fs = require("node:fs");
const path = require("node:path");
const [mode, start, tool] = process.argv.slice(1);

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

function isFile(file) {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

function packageEntry(dir, name, bin) {
  const packageDir = path.join(dir, "node_modules", name);
  const pkg = readJson(path.join(packageDir, "package.json"));
  const entry = path.join(packageDir, "bin", bin);
  // Standalone dependencies can be npm aliases. Only Vite+ identity requires
  // the package-name validation specified by the detection RFC.
  return (name !== "vite-plus" || pkg?.name === name) && isFile(entry) ? entry : null;
}

function ancestors(start) {
  const directories = [];
  let dir = path.resolve(start);
  while (true) {
    const pkg = readJson(path.join(dir, "package.json"));
    const boundary = fs.existsSync(path.join(dir, "pnpm-workspace.yaml")) ||
      fs.existsSync(path.join(dir, "lerna.json")) ||
      (pkg !== null && Object.hasOwn(pkg, "workspaces"));
    directories.push({
      root: dir,
      package: pkg,
      vp: packageEntry(dir, "vite-plus", "vp"),
      standalone: packageEntry(dir, tool, tool),
    });
    const parent = path.dirname(dir);
    if (boundary || dir === parent) return directories;
    dir = parent;
  }
}

function readHeader(file) {
  // Read only the beginning: vp may be a large native executable.
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.alloc(8192);
  let header;
  try {
    header = buffer.subarray(0, fs.readSync(fd, buffer, 0, buffer.length, 0)).toString();
  } finally {
    fs.closeSync(fd);
  }
  return header;
}

function executable(file) {
  if (!isFile(file)) return null;
  const real = fs.realpathSync(file);
  const header = readHeader(real);
  if (/^#![^\r\n]*\bnode\b/.test(header) || /\.[cm]?js$/i.test(real)) {
    return { path: real, node: true };
  }

  // npm and pnpm record the Node entry in their POSIX and Windows shims.
  // Resolve that entry, including symlinks to global pnpm shims, instead of
  // passing a shell script to Node or requiring cmd.exe on Windows.
  const entryPattern = /["']([^"'\r\n]+)["']/g;
  for (const match of header.matchAll(entryPattern)) {
    const target = match[1].replace(/\$basedir|%~dp0|%dp0%/g, path.dirname(real) + path.sep);
    if (!path.isAbsolute(target) || path.resolve(target) === real || !isFile(target)) continue;
    const text = readHeader(target);
    if (/^#![^\r\n]*\bnode\b/.test(text)) return { path: path.resolve(target), node: true };
  }
  if (/\.(cmd|bat)$/i.test(real)) {
    throw new Error("Cannot resolve the vp shim. Set vpPath to vite-plus/bin/vp.");
  }
  return { path: file, node: false };
}

try {
  let result;
  if (mode === "ancestors") {
    result = ancestors(start);
  } else if (mode === "global") {
    result = null;
    const pathKey = Object.keys(process.env).find((key) => key.toUpperCase() === "PATH");
    const names = process.platform === "win32" ? ["vp.cmd", "vp.exe", "vp"] : ["vp"];
    for (const dir of (process.env[pathKey] || "").split(path.delimiter).filter(Boolean)) {
      for (const name of names) {
        result = executable(path.resolve(dir, name));
        if (result) break;
      }
      if (result) break;
    }
  } else {
    result = executable(path.resolve(start, tool));
  }
  process.stdout.write(JSON.stringify(result));
} catch (error) {
  process.stderr.write(String(error));
  process.exitCode = 1;
}
