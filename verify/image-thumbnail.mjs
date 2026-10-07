/**
 * Durable probe: an image file's icon must be its own picture, not the generic image glyph.
 *
 * Adds a real floatie per image file and reads the icon the record actually holds — the same
 * path a user's photo takes when they drop it on the desktop. The bytes compared are the
 * ones the tile draws.
 *
 * IT MAKES ITS OWN IMAGES. It never points at a file you already had: `floaty_add_launcher`
 * *moves* the file it is given into the launcher's root (that is what makes a drag work),
 * so a probe that borrows your pictures destroys them. Every file here is generated into a
 * temp folder and removed on the way out.
 *
 * Usage: node verify/image-thumbnail.mjs [port]
 */
import { APP_PORT } from "./cdp.mjs";
import { attach, flag, probe } from "./runtime.mjs";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join } from "node:path";

const port = Number(flag("port", process.argv[3] ?? APP_PORT));
const { check, skip, finish } = probe("image-thumbnail");
const app = await attach({ port }).catch((e) => skip(e.message));

/** PNG dimensions, from the IHDR: width at byte 16, height at 20. */
function pngSize(bytes) {
  // Signature is 89 50 4E 47 0D 0A 1A 0A; check the middle four, which is unambiguous.
  if (bytes.length < 24 || !bytes.subarray(1, 4).equals(Buffer.from([0x50, 0x4e, 0x47]))) {
    return null;
  }
  return { w: bytes.readUInt32BE(16), h: bytes.readUInt32BE(20) };
}

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return c ^ -1;
}

/**
 * A real PNG of the given size, filled with a gradient seeded from its own colour, so no
 * two cases hold the same bytes: "every file produced the same icon" then means one glyph
 * reused, which is the bug, rather than two inputs that were already identical.
 */
function makePng(w, h, [r, g, b]) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body) >>> 0);
    return Buffer.concat([len, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // colour type: RGBA
  const scan = [];
  for (let y = 0; y < h; y++) {
    const row = Buffer.alloc(1 + w * 4);
    row[0] = 0; // filter: none
    for (let x = 0; x < w; x++) {
      row[1 + x * 4] = (r + x) & 0xff;
      row[2 + x * 4] = (g + y) & 0xff;
      row[3 + x * 4] = b;
      row[4 + x * 4] = 0xff;
    }
    scan.push(row);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(Buffer.concat(scan))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// Sizes chosen to cover the three cases the resolver has to get right: under the cap, at it,
// and over it. Each is a different colour, so identical output bytes mean one glyph reused.
const CASES = [
  { name: "under-cap-square.png", w: 96, h: 96, colour: [200, 40, 40] },
  { name: "under-cap-wide.png", w: 320, h: 180, colour: [40, 200, 90] },
  { name: "at-cap.png", w: 256, h: 256, colour: [40, 90, 220] },
  { name: "over-cap-tall.png", w: 300, h: 600, colour: [230, 180, 30] },
];

const dir = mkdtempSync(join(tmpdir(), "floaty-image-probe-"));
const added = [];
try {
  for (const c of CASES) {
    const file = join(dir, c.name);
    writeFileSync(file, makePng(c.w, c.h, c.colour));
    try {
      const rec = await app.invoke("floaty_add_launcher", {
        name: `probe-${c.name}`,
        path: file,
      });
      // The record comes back without an icon: resolution is lazy, and this is the call that
      // does it — the same one a widget makes when it mounts.
      const icon = await app.invoke("floaty_icon", { id: rec.id });
      added.push({ ...c, id: rec.id, icon: icon ?? "" });
    } catch (e) {
      check(false, `${c.name}: could not be added as a floatie — ${e}`);
    }
  }

  console.log(`image-thumbnail: ${added.length} generated image(s)\n`);

  const GENERIC = 32; // the shell's type icon is 32px whatever the file is
  const stored = [];

  for (const { name, w, h, icon } of added) {
    if (!icon || icon === "none") {
      check(false, `${name}: the record holds no icon at all`);
      continue;
    }
    const bytes = await app.page(0).evaluate(`(async () => {
      const res = await fetch(${JSON.stringify(icon)});
      if (!res.ok) return null;
      return Array.from(new Uint8Array(await res.arrayBuffer()));
    })()`);
    if (!bytes) {
      check(false, `${name}: its icon url did not serve bytes`);
      continue;
    }
    const buf = Buffer.from(bytes);
    const got = pngSize(buf);
    if (!got) {
      check(false, `${name}: its icon is not a readable png`);
      continue;
    }
    stored.push({ name, got, len: buf.length });
    console.log(
      `  ${name.padEnd(22)} file ${String(w).padStart(4)}x${String(h).padEnd(4)}` +
        `  icon ${String(got.w).padStart(3)}x${String(got.h).padEnd(4)}  ${buf.length}b`
    );

    // The generic glyph is 32px whatever the file is; a picture comes back bigger.
    check(
      got.w > GENERIC && got.h > GENERIC,
      `${name}: its own picture at ${got.w}x${got.h}, not the ${GENERIC}px type icon`
    );
    // Downscaled to fit 256 on the long edge, never above it.
    check(
      Math.max(got.w, got.h) <= 256,
      `${name}: ${got.w}x${got.h}, within 256 on the long edge`
    );
    // The shape has to be the file's own: a wide file must not come back square.
    check(
      Math.abs(got.w / got.h - w / h) < 0.06,
      `${name}: ${got.w}x${got.h} keeps the file's ${w}x${h} proportions`
    );
    // And the scale: the file's long edge, capped at 256.
    const wantLong = Math.min(256, Math.max(w, h));
    const gotLong = Math.max(got.w, got.h);
    check(
      gotLong === wantLong,
      `${name}: long edge ${gotLong} is the ${wantLong} a ${Math.max(w, h)}px file calls for`
    );
  }

  // Distinct files must not share a picture: the generic glyph is one image for every
  // file, which is how this bug first presented as "the cache keeps returning one icon".
  if (stored.length >= 2) {
    const lens = new Set(stored.map((s) => s.len));
    check(
      lens.size > 1,
      `${stored.length} different images each stored their own bytes (${lens.size} distinct)`
    );
  }
} finally {
  // Leave the desktop as it was found. These records exist only to prove the point.
  for (const { id } of added) await app.invoke("floaty_delete", { id }).catch(() => {});
  rmSync(dir, { recursive: true, force: true });
  await app.close();
}
finish();