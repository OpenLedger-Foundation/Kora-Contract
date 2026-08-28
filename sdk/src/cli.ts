#!/usr/bin/env node

const [, , command = "help"] = process.argv;

if (command === "help") {
  console.log("kora-sdk cli: help | addresses");
}

if (command === "addresses") {
  console.log("Use loadKoraAddresses() from the SDK deployment helpers.");
}
