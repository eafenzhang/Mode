// Register (or unregister) the Native Messaging host for the Mode extension.
//
// Writes the host manifest under %LOCALAPPDATA%\Mode\browser-import\native-host and registers
// HKCU\Software\Google\Chrome\NativeMessagingHosts\com.mode.browser_import.
//
// Usage:
//   pnpm --filter @mode/browser-import-extension register-native-host
//   pnpm --filter @mode/browser-import-extension unregister-native-host
//
// The host executable must exist; build it with:
//   node scripts/rust-cargo.ts build --release -p browser-import-host
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = join(packageRoot, "..", "..");
const manifestTemplate = join(packageRoot, "native-host", "com.mode.browser_import.json");
const extensionManifest = JSON.parse(
  readFileSync(join(packageRoot, "src", "manifest.json"), "utf8"),
) as { key?: string };
const hostName = "com.mode.browser_import";
const registryKey = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${hostName}`;

if (process.platform !== "win32") {
  console.log("[browser-import-extension] native host registration is Windows-only");
  process.exit(0);
}

const unregister = process.argv.includes("--unregister");
if (unregister) {
  try {
    execFileSync("reg", ["delete", registryKey, "/f"], { stdio: "ignore" });
  } catch {
    // Already absent.
  }
  const manifestDir = join(
    process.env.LOCALAPPDATA ?? process.cwd(),
    "Mode",
    "browser-import",
    "native-host",
  );
  rmSync(manifestDir, { recursive: true, force: true });
  console.log(`[browser-import-extension] unregistered ${hostName}`);
  process.exit(0);
}

if (!extensionManifest.key) {
  console.error("[browser-import-extension] src/manifest.json is missing the fixed key");
  process.exit(1);
}
const extensionId = computeExtensionId(extensionManifest.key);

const hostCandidates = [
  process.env.MODE_BROWSER_IMPORT_HOST,
  join(repoRoot, "target", "release", "mode-browser-import-host.exe"),
  join(repoRoot, "target", "debug", "mode-browser-import-host.exe"),
].filter((value): value is string => Boolean(value));
const hostPath = hostCandidates.find((candidate) => existsSync(candidate));
if (!hostPath) {
  console.error(
    "[browser-import-extension] native host executable not found; build it with:\n" +
      "  node scripts/rust-cargo.ts build --release -p browser-import-host",
  );
  process.exit(1);
}

const manifest = JSON.parse(readFileSync(manifestTemplate, "utf8")) as {
  allowed_origins: string[];
  path: string;
};
manifest.path = hostPath;
manifest.allowed_origins = [`chrome-extension://${extensionId}/`];

const manifestDir = join(
  process.env.LOCALAPPDATA ?? process.cwd(),
  "Mode",
  "browser-import",
  "native-host",
);
mkdirSync(manifestDir, { recursive: true });
const manifestPath = join(manifestDir, `${hostName}.json`);
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
execFileSync("reg", ["add", registryKey, "/ve", "/t", "REG_SZ", "/d", manifestPath, "/f"], {
  stdio: "inherit",
});

console.log(`[browser-import-extension] registered ${hostName}`);
console.log(`  extension id: ${extensionId}`);
console.log(`  host: ${hostPath}`);
console.log(`  manifest: ${manifestPath}`);
console.log(`  load unpacked: ${join(packageRoot, "dist")}`);

/** Chrome derives the extension ID from SHA-256(public key DER), first 16 bytes -> a-p. */
function computeExtensionId(publicKeyBase64: string): string {
  const digest = createHash("sha256").update(Buffer.from(publicKeyBase64, "base64")).digest("hex");
  return digest
    .slice(0, 32)
    .split("")
    .map((nibble) => String.fromCharCode(97 + Number.parseInt(nibble, 16)))
    .join("");
}
