#!/usr/bin/env node
import { readFileSync } from "node:fs";
import parse from "@changesets/parse";

const contents = readFileSync(0, "utf8");

try {
  const result = parse(contents);
  process.stdout.write(JSON.stringify(result));
} catch (err) {
  process.stderr.write(String(err?.stack ?? err));
  process.exit(1);
}
