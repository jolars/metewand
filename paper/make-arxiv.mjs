import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const paper = dirname(fileURLToPath(import.meta.url));
if (/\\\[@/.test(readFileSync(join(paper, "paper.md"), "utf8"))) {
  throw new Error("Escaped citations in paper.md. Use Pandoc syntax: [@key].");
}

const work = mkdtempSync(join(tmpdir(), "metewand-arxiv-"));
const source = join(work, "source");
const build = join(work, "build");
const image = "openjournals/inara";
const rerunWarning = /Rerun to get|Please (?:re)?run/;
const undefinedReference =
  /undefined references|undefined citations|Citation .* undefined|Reference .* undefined/;
const dockerArgs = ["run", "--rm", "--volume", `${work}:/data`];
if (process.getuid && process.getgid) {
  dockerArgs.push("--user", `${process.getuid()}:${process.getgid()}`);
}
dockerArgs.push("--env", "JOURNAL=joss");

function run(command, args) {
  execFileSync(command, args, { stdio: "inherit" });
}

try {
  mkdirSync(source);
  mkdirSync(build);
  copyFileSync(join(paper, "paper.md"), join(work, "paper.md"));
  copyFileSync(join(paper, "paper.bib"), join(work, "paper.bib"));
  run("docker", [
    ...dockerArgs,
    "--workdir",
    "/data",
    "--entrypoint",
    "pandoc",
    image,
    "--citeproc",
    "--fail-if-warnings",
    "--to=plain",
    "--output=/dev/null",
    "paper.md",
  ]);
  run("docker", [...dockerArgs, image, "-o", "preprint", "paper.md"]);

  // Inara embeds the formatted bibliography, so the upload needs no Biber run.
  const tex = readFileSync(join(work, "paper.preprint.tex"), "utf8");
  writeFileSync(join(source, "main.tex"), tex);
  const figures = new Set();
  for (const match of tex.matchAll(
    /\\includegraphics(?:\[[^\]]*\])?\{([^}]+)\}/g,
  )) {
    const figure = match[1];
    const local = relative(paper, resolve(paper, figure));
    if (isAbsolute(figure) || local === ".." || local.startsWith(`..${sep}`)) {
      throw new Error(`Figure must be inside paper/: ${figure}`);
    }
    const destination = join(source, local);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(join(paper, local), destination);
    figures.add(local);
  }

  // Compile only the upload files so undeclared local dependencies cannot help.
  let log;
  for (let pass = 0; pass < 5; pass++) {
    run("docker", [
      ...dockerArgs,
      "--workdir",
      "/data/source",
      "--entrypoint",
      "pdflatex",
      image,
      "-no-shell-escape",
      "-interaction=nonstopmode",
      "-halt-on-error",
      "-output-directory=/data/build",
      "main.tex",
    ]);
    log = readFileSync(join(build, "main.log"), "utf8").replace(/\r?\n/g, " ");
    if (!rerunWarning.test(log) && !undefinedReference.test(log)) break;
  }
  if (undefinedReference.test(log)) {
    throw new Error("The arXiv build has unresolved references or citations.");
  }
  if (rerunWarning.test(log)) {
    throw new Error("The arXiv build did not stabilize after five LaTeX passes.");
  }

  run("tar", [
    "-czf",
    join(work, "arxiv.tar.gz"),
    "-C",
    source,
    "main.tex",
    ...figures,
  ]);
  copyFileSync(join(work, "arxiv.tar.gz"), join(paper, "arxiv.tar.gz"));
  copyFileSync(join(build, "main.pdf"), join(paper, "arxiv.pdf"));
  console.log(`Upload: ${join(paper, "arxiv.tar.gz")}`);
  console.log(`Preview: ${join(paper, "arxiv.pdf")}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
