// Renders PNGs that browsers and social sites need from the SVG sources.
import { readFileSync, writeFileSync } from "node:fs";
import { Resvg } from "@resvg/resvg-js";

const jobs = [
  ["og.svg", "../public/og.png", 1200],
  ["x-header.svg", "../public/brand/x-header.png", 1500],
  ["apple-touch-icon.svg", "../public/apple-touch-icon.png", 180],
  ["app-icon.svg", "../public/brand/icon-512.png", 512],
];

for (const [source, target, width] of jobs) {
  const svg = readFileSync(new URL(source, import.meta.url));
  const png = new Resvg(svg, {
    fitTo: { mode: "width", value: width },
    font: { loadSystemFonts: true },
  }).render().asPng();
  writeFileSync(new URL(target, import.meta.url), png);
  console.log(`${target}: ${png.length} bytes`);
}

// favicon.ico for crawlers and browsers that only look at /favicon.ico. Each
// size is a PNG inside the ICO container, rendered from the SVG favicon.
const sizes = [16, 32, 48];
const images = sizes.map((size) =>
  new Resvg(readFileSync(new URL("../public/favicon.svg", import.meta.url)), {
    fitTo: { mode: "width", value: size },
  }).render().asPng(),
);
const header = Buffer.alloc(6 + 16 * sizes.length);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
sizes.forEach((size, i) => {
  const entry = 6 + 16 * i;
  header.writeUInt8(size, entry);
  header.writeUInt8(size, entry + 1);
  header.writeUInt16LE(1, entry + 4);
  header.writeUInt16LE(32, entry + 6);
  header.writeUInt32LE(images[i].length, entry + 8);
  header.writeUInt32LE(offset, entry + 12);
  offset += images[i].length;
});
const ico = Buffer.concat([header, ...images]);
writeFileSync(new URL("../public/favicon.ico", import.meta.url), ico);
console.log(`../public/favicon.ico: ${ico.length} bytes`);
