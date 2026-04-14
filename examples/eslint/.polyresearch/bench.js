/* eslint-disable no-console -- benchmark output */
"use strict";

const { Linter } = require("../lib/linter");
const { ESLint } = require("../lib/eslint");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");

const repoRoot = path.resolve(__dirname, "..");

/*
 * ============================================================================
 * Part A: Single-file Linter.verify() benchmark (recommended rules)
 * ============================================================================
 */

const code = fs.readFileSync(
	path.join(repoRoot, "tests/bench/large.js"),
	"utf8",
);
const recommended = require("../packages/js/src/configs/eslint-recommended.js");
const config = [recommended];
const linter = new Linter();

const WARMUP_A = 3;
const RUNS_A = 10;

for (let i = 0; i < WARMUP_A; i++) {
	linter.verify(code, config, { filename: "large.js" });
}

const timesA = [];
let messages;

for (let i = 0; i < RUNS_A; i++) {
	const start = process.hrtime.bigint();

	messages = linter.verify(code, config, { filename: "large.js" });
	const end = process.hrtime.bigint();

	timesA.push(Number(end - start) / 1e6);
}

timesA.sort((a, b) => a - b);
const medianA = timesA[Math.floor(timesA.length / 2)];

const msgFingerprint = crypto
	.createHash("sha256")
	.update(
		JSON.stringify(
			messages.map(m => ({
				ruleId: m.ruleId,
				line: m.line,
				column: m.column,
				severity: m.severity,
			})),
		),
	)
	.digest("hex")
	.slice(0, 12);

/*
 * ============================================================================
 * Part B: Multi-file project lint (ESLint linting its own lib/)
 * ============================================================================
 */

const WARMUP_B = 1;
const RUNS_B = 5;

/**
 * Lint the lib/ directory and return summary counts.
 * @returns {Promise<{fileCount: number, errorCount: number, warningCount: number}>} Results.
 */
async function lintLib() {
	const eslint = new ESLint({
		cwd: repoRoot,
		cache: false,
	});
	const results = await eslint.lintFiles(["lib/"]);
	let errorCount = 0;
	let warningCount = 0;

	for (const r of results) {
		errorCount += r.errorCount;
		warningCount += r.warningCount;
	}
	return { fileCount: results.length, errorCount, warningCount };
}

/**
 * Run Workload B multiple times and return timing results.
 * @returns {Promise<{medianB: number, timesB: number[], lastResult: {fileCount: number, errorCount: number, warningCount: number}}>} Results.
 */
async function runPartB() {
	for (let i = 0; i < WARMUP_B; i++) {
		await lintLib();
	}

	const timesB = [];
	let lastResult;

	for (let i = 0; i < RUNS_B; i++) {
		const start = process.hrtime.bigint();

		lastResult = await lintLib();
		const end = process.hrtime.bigint();

		timesB.push(Number(end - start) / 1e6);
	}

	timesB.sort((a, b) => a - b);
	const medianB = timesB[Math.floor(timesB.length / 2)];

	return { medianB, timesB, lastResult };
}

runPartB().then(({ medianB, timesB, lastResult }) => {
	/*
	 * Composite: both metrics on comparable scale.
	 * A is ~175ms, B is ~4500ms. We weight B/25 so both contribute roughly equally.
	 */
	const composite = medianA + medianB / 25;

	console.log(`METRIC_A: ${medianA.toFixed(2)}`);
	console.log(`METRIC_A_MIN: ${Math.min(...timesA).toFixed(2)}`);
	console.log(`METRIC_A_MAX: ${Math.max(...timesA).toFixed(2)}`);
	console.log(`MESSAGES: ${messages.length}`);
	console.log(`FINGERPRINT: ${msgFingerprint}`);
	console.log(``);
	console.log(`METRIC_B: ${medianB.toFixed(2)}`);
	console.log(`METRIC_B_MIN: ${Math.min(...timesB).toFixed(2)}`);
	console.log(`METRIC_B_MAX: ${Math.max(...timesB).toFixed(2)}`);
	console.log(`FILES: ${lastResult.fileCount}`);
	console.log(`ERRORS: ${lastResult.errorCount}`);
	console.log(`WARNINGS: ${lastResult.warningCount}`);
	console.log(``);
	console.log(`METRIC: ${composite.toFixed(2)}`);
});
/* eslint-enable no-console -- re-enable after benchmark output */
