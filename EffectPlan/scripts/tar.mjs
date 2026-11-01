/**
 * A minimal tar reader, so the size gate can measure the real bytes of the
 * packed artifact on every platform. GNU tar reads a Windows path as a remote
 * host spec, and the gate must not depend on the host's tar build.
 */

import { gunzipSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const BLOCK = 512;

/** Reads one NUL-terminated or space-terminated field. */
function field(buffer, offset, length) {
  const slice = buffer.subarray(offset, offset + length);
  const end = slice.indexOf(0);
  const text = slice.subarray(0, end < 0 ? slice.length : end).toString("utf8").trim();
  return text;
}

/** Reads an octal field, tolerating the base-256 encoding GNU tar also emits. */
function octal(buffer, offset, length) {
  if (length > 0 && (buffer[offset] & 0x80) !== 0) {
    let value = 0;
    for (let i = offset + 1; i < offset + length; i++) {
      value = value * 256 + buffer[i];
    }
    return value;
  }
  const text = field(buffer, offset, length);
  return text.length === 0 ? 0 : Number.parseInt(text, 8);
}

/**
 * Extracts a gzipped tar archive into `destination`.
 * Returns the entries written, with the bytes each one occupies on disk.
 */
export function extractTarGz(archive, destination) {
  const raw = gunzipSync(archive);
  const written = [];
  let offset = 0;

  while (offset + BLOCK <= raw.length) {
    const header = raw.subarray(offset, offset + BLOCK);
    if (header.every((byte) => byte === 0)) break; // end-of-archive marker
    offset += BLOCK;

    const name = field(header, 0, 100);
    const size = octal(header, 124, 12);
    const type = String.fromCharCode(header[156]);
    const prefix = field(header, 345, 155);
    const link = field(header, 157, 100);
    const data = raw.subarray(offset, offset + size);
    offset += Math.ceil(size / BLOCK) * BLOCK;

    if (type === "5") {
      mkdirSync(path.join(destination, name), { recursive: true });
      continue;
    }
    if (type === "l" || type === "k") {
      continue; // symbolic link and hard link records carry no payload
    }
    if (type !== "0" && type !== "\0" && type !== "7") {
      continue; // anything that is not a regular file
    }

    const relative = prefix.length > 0 ? `${prefix}/${name}` : name;
    const target = path.join(destination, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, data);
    written.push({ path: relative, bytes: data.length, link: link.length > 0 ? link : undefined });
  }

  return written;
}
