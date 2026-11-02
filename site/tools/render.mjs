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
