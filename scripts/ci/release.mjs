#!/usr/bin/env node
// Release tooling used by .github/workflows/release.yml (and runnable locally).
// No dependencies beyond Node >= 18 and GNU tar + gzip on PATH.
//
//   pack         dist/<target>/tokenade[.exe] -> tokenade-<v>-<target>.tar.gz (+ .sha256)
//   manifest     signed-update manifest (v=1 schema read by crates/tokenade-core/src/updater.rs)
//   sign         detached Ed25519 signature -> <file>.sig (base64url, no padding)
//   verify       check <file>.sig against the public key embedded in the CLI
//   pack-model   model-<MODEL_VERSION>.tar.gz from the files listed in model_fetch.rs
//   npm-platform assemble the per-platform npm packages (@tokenade/cli-<os>-<cpu>)
//   check-version tag / Cargo.toml / npm/package.json agree
//
// The signing key is a raw 32-byte Ed25519 seed, hex, read from the env var
// TOKENADE_UPDATE_SIGNING_KEY or from the file named by --key-file. It is never
// written anywhere and never printed.

import { spawnSync } from "node:child_process";
import { createHash, createPrivateKey, createPublicKey, sign, verify } from "node:crypto";
import {
	chmodSync, copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

// Rust target -> npm platform package. Must match npm/platform.js PACKAGE_BY_TARGET.
export const PLATFORMS = {
	"x86_64-unknown-linux-gnu": { name: "@tokenade/cli-linux-x64", os: "linux", cpu: "x64", libc: "glibc" },
	"x86_64-unknown-linux-musl": { name: "@tokenade/cli-linux-x64-musl", os: "linux", cpu: "x64", libc: "musl" },
	"aarch64-unknown-linux-musl": { name: "@tokenade/cli-linux-arm64-musl", os: "linux", cpu: "arm64", libc: "musl" },
	"aarch64-apple-darwin": { name: "@tokenade/cli-darwin-arm64", os: "darwin", cpu: "arm64" },
	"x86_64-apple-darwin": { name: "@tokenade/cli-darwin-x64", os: "darwin", cpu: "x64" },
	"x86_64-pc-windows-gnu": { name: "@tokenade/cli-win32-x64", os: "win32", cpu: "x64" },
	"aarch64-pc-windows-gnullvm": { name: "@tokenade/cli-win32-arm64", os: "win32", cpu: "arm64" },
};

const binName = (target) => (target.includes("windows") ? "tokenade.exe" : "tokenade");
const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

function args(argv) {
	const out = { _: [] };
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i];
		if (a.startsWith("--")) {
			const [k, v] = a.slice(2).split("=", 2);
			out[k] = v ?? (argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : true);
		} else out._.push(a);
	}
	return out;
}

function need(o, k) {
	if (!o[k] || o[k] === true) throw new Error(`missing --${k}`);
	return o[k];
}

function run(cmd, argv, opts = {}) {
	const r = spawnSync(cmd, argv, { stdio: "inherit", ...opts });
	if (r.status !== 0) throw new Error(`${cmd} ${argv.join(" ")} failed (exit ${r.status})`);
}

// Deterministic single-entry tarball: what updater/swap.rs `read_single_tar_entry`
// and npm/install.js extract. Fixed mtime/owner, ustar, gzip -n.
function deterministicTarGz(srcDir, entries, outPath) {
	const plain = outPath.replace(/\.gz$/, "");
	rmSync(plain, { force: true });
	rmSync(outPath, { force: true });
	run("tar", ["--sort=name", "--mtime=@0", "--owner=0", "--group=0", "--numeric-owner",
		"--format=ustar", "-cf", plain, "-C", srcDir, ...entries]);
	run("gzip", ["-n", "-9", "-f", plain]);
}

export function readWorkspaceVersion() {
	const toml = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
	const ws = toml.split(/^\[workspace\.package\]\s*$/m)[1] ?? "";
	const m = ws.match(/^\s*version\s*=\s*"([^"]+)"/m);
	if (!m) throw new Error("no [workspace.package] version in Cargo.toml");
	return m[1];
}

export function embeddedPubkeyHex() {
	const src = readFileSync(join(ROOT, "crates/tk-base/src/signing.rs"), "utf8");
	const m = src.match(/RELEASE_PUBKEY_HEX:\s*&str\s*=\s*"([0-9a-f]{64})"/);
	if (!m) throw new Error("RELEASE_PUBKEY_HEX not found in crates/tk-base/src/signing.rs");
	return m[1];
}

function loadSeed(o) {
	let hex = o["key-file"] ? readFileSync(o["key-file"], "utf8") : process.env.TOKENADE_UPDATE_SIGNING_KEY ?? "";
	hex = hex.replace(/^0x/i, "").replace(/\s+/g, "");
	if (!/^[0-9a-fA-F]{64}$/.test(hex)) {
		throw new Error("signing key must be 32 bytes hex (TOKENADE_UPDATE_SIGNING_KEY or --key-file)");
	}
	return Buffer.from(hex, "hex");
}

// Raw Ed25519 keys <-> Node KeyObjects (PKCS#8 / SPKI DER prefixes, RFC 8410).
export const privateKeyFromSeed = (seed) =>
	createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]), format: "der", type: "pkcs8" });
export const publicKeyFromRaw = (raw) =>
	createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw]), format: "der", type: "spki" });

export function signBytes(bytes, seed) {
	return sign(null, bytes, privateKeyFromSeed(seed)).toString("base64url"); // no padding
}

export function verifyBytes(bytes, sigText, pubHex) {
	const sig = Buffer.from(sigText.trim().replace(/=+$/, ""), "base64url");
	return sig.length === 64 && verify(null, bytes, publicKeyFromRaw(Buffer.from(pubHex, "hex")), sig);
}

// The manifest's canonical bytes are exactly what is signed and served; the
// client verifies the fetched bytes verbatim. Key order is fixed here.
export function canonicalManifest({ version, releasedAt, minSupportedVersion, targets }) {
	const sorted = Object.fromEntries(Object.keys(targets).sort().map((k) => [k, targets[k]]));
	const obj = { v: 1, version, releasedAt };
	if (minSupportedVersion) obj.minSupportedVersion = minSupportedVersion;
	obj.targets = sorted;
	return JSON.stringify(obj);
}

const commands = {
	pack(o) {
		const dist = resolve(need(o, "dist"));
		const version = need(o, "version");
		const out = resolve(need(o, "out"));
		mkdirSync(out, { recursive: true });
		const targets = readdirSync(dist).filter((d) => statSync(join(dist, d)).isDirectory());
		if (targets.length === 0) throw new Error(`no target dirs in ${dist}`);
		for (const target of targets.sort()) {
			const bin = binName(target);
			if (!existsSync(join(dist, target, bin))) throw new Error(`missing ${join(dist, target, bin)}`);
			const name = `tokenade-${version}-${target}.tar.gz`;
			deterministicTarGz(join(dist, target), [bin], join(out, name));
			const h = sha256(readFileSync(join(out, name)));
			writeFileSync(join(out, `${name}.sha256`), `${h}  ${name}\n`);
			console.log(`packed ${name} sha256:${h}`);
		}
	},

	manifest(o) {
		const out = resolve(need(o, "out"));
		const version = need(o, "version");
		const baseUrl = need(o, "base-url").replace(/\/+$/, "");
		const re = new RegExp(`^tokenade-${version.replace(/\./g, "\\.")}-(.+)\\.tar\\.gz$`);
		const targets = {};
		for (const f of readdirSync(out)) {
			const m = f.match(re);
			if (!m) continue;
			const bytes = readFileSync(join(out, f));
			targets[m[1]] = { url: `${baseUrl}/${f}`, sha256: sha256(bytes), size: bytes.length };
		}
		if (o["require-all"]) {
			const missing = Object.keys(PLATFORMS).filter((t) => !targets[t]);
			if (missing.length) throw new Error(`manifest would miss targets: ${missing.join(", ")}`);
		}
		const releasedAt = o["released-at"] ? Number(o["released-at"]) : Math.floor(Date.now() / 1000);
		const text = canonicalManifest({ version, releasedAt, minSupportedVersion: o["min-supported"], targets });
		writeFileSync(join(out, "manifest.json"), text);
		console.log(`manifest.json: ${Object.keys(targets).length} target(s), ${text.length} bytes`);
	},

	sign(o) {
		const seed = loadSeed(o);
		// Refuse a key whose public half is not the one the CLI embeds: a
		// manifest signed with it would be rejected by every client.
		const derived = createPublicKey(privateKeyFromSeed(seed)).export({ format: "der", type: "spki" }).subarray(-32).toString("hex");
		if (!o["any-key"] && derived !== embeddedPubkeyHex()) {
			throw new Error("signing key does not match RELEASE_PUBKEY_HEX in crates/tk-base/src/signing.rs (pass --any-key to override)");
		}
		for (const f of o._) {
			const bytes = readFileSync(f);
			writeFileSync(`${f}.sig`, signBytes(bytes, seed));
			console.log(`signed ${basename(f)} -> ${basename(f)}.sig`);
		}
	},

	verify(o) {
		const pubHex = typeof o.pubkey === "string" ? o.pubkey : embeddedPubkeyHex();
		let bad = 0;
		for (const f of o._) {
			const ok = verifyBytes(readFileSync(f), readFileSync(`${f}.sig`, "utf8"), pubHex);
			console.log(`${ok ? "OK " : "BAD"} ${basename(f)}.sig (pubkey ${pubHex.slice(0, 16)}…)`);
			if (!ok) bad++;
		}
		if (bad) process.exitCode = 1;
	},

	"pack-model"(o) {
		const src = readFileSync(join(ROOT, "crates/tokenade-core/src/model_fetch.rs"), "utf8");
		const version = src.match(/pub const MODEL_VERSION:\s*&str\s*=\s*"([^"]+)"/)[1];
		const files = [...src.match(/const MODEL_FILES:\s*\[&str;\s*\d+\]\s*=\s*\[([\s\S]*?)\];/)[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
		const digests = new Map([...src.match(/const MODEL_FILE_SHA256:[^=]*=\s*\[([\s\S]*?)\n\];/)[1]
			.matchAll(/"([^"]+)"\s*,\s*"([0-9a-f]{64})"/g)].map((p) => [p[1], p[2]]));
		const dir = resolve(need(o, "src"));
		const out = resolve(need(o, "out"));
		mkdirSync(out, { recursive: true });
		for (const f of files) {
			const want = digests.get(f);
			const got = sha256(readFileSync(join(dir, f)));
			if (want && got !== want) throw new Error(`${f}: sha256 ${got} != baked ${want}`);
		}
		const name = `model-${version}.tar.gz`;
		deterministicTarGz(dir, files, join(out, name));
		console.log(`packed ${name} (${files.length} files, digests OK); sign it with: release.mjs sign ${join(out, name)}`);
	},

	"npm-platform"(o) {
		const dist = resolve(need(o, "dist"));
		const version = need(o, "version");
		const out = resolve(need(o, "out"));
		for (const [target, p] of Object.entries(PLATFORMS)) {
			const bin = binName(target);
			const srcBin = join(dist, target, bin);
			if (!existsSync(srcBin)) throw new Error(`missing ${srcBin}`);
			const dir = join(out, p.name.replace("@tokenade/", ""));
			rmSync(dir, { recursive: true, force: true });
			mkdirSync(dir, { recursive: true });
			const pkg = {
				name: p.name,
				version,
				description: `Tokenade CLI binary for ${p.os}/${p.cpu}${p.libc ? `/${p.libc}` : ""}. Installed automatically as an optionalDependency of @tokenade/cli.`,
				homepage: "https://github.com/pi-infected/tokenade",
				repository: { type: "git", url: "git+https://github.com/pi-infected/tokenade.git", directory: "npm" },
				license: "MIT OR Apache-2.0",
				os: [p.os],
				cpu: [p.cpu],
				...(p.libc ? { libc: [p.libc] } : {}),
				files: [bin, "LICENSE-MIT", "LICENSE-APACHE"],
				publishConfig: { access: "public" },
			};
			writeFileSync(join(dir, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
			copyFileSync(srcBin, join(dir, bin));
			if (!bin.endsWith(".exe")) chmodSync(join(dir, bin), 0o755);
			for (const l of ["LICENSE-MIT", "LICENSE-APACHE"]) copyFileSync(join(ROOT, l), join(dir, l));
			console.log(`assembled ${p.name}@${version} in ${dir}`);
		}
	},

	"check-version"(o) {
		const tag = String(need(o, "tag")).replace(/^refs\/tags\//, "");
		const cargo = readWorkspaceVersion();
		const npm = JSON.parse(readFileSync(join(ROOT, "npm/package.json"), "utf8"));
		const errs = [];
		if (tag !== `v${cargo}`) errs.push(`tag ${tag} != Cargo.toml v${cargo}`);
		if (npm.version !== cargo) errs.push(`npm/package.json ${npm.version} != ${cargo}`);
		for (const [k, v] of Object.entries(npm.optionalDependencies ?? {})) if (v !== cargo) errs.push(`${k} pinned ${v}`);
		const want = Object.values(PLATFORMS).map((p) => p.name).sort();
		if (JSON.stringify(Object.keys(npm.optionalDependencies ?? {}).sort()) !== JSON.stringify(want)) {
			errs.push("npm optionalDependencies != platform package list");
		}
		if (errs.length) throw new Error(errs.join("\n"));
		console.log(`version ${cargo} consistent (tag, Cargo.toml, npm)`);
	},
};

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
	const [cmd, ...rest] = process.argv.slice(2);
	try {
		if (!commands[cmd]) throw new Error(`usage: release.mjs <${Object.keys(commands).join("|")}> [options]`);
		commands[cmd](args(rest));
	} catch (e) {
		console.error(`✗ ${e.message}`);
		process.exit(1);
	}
}
