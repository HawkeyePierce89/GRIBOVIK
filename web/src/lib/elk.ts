import ELK from "elkjs/lib/elk.bundled.js";

let failWorker: (error: Error) => void;
const failed = new Promise<never>((_, reject) => {
  failWorker = reject;
});

export function workerFailure(): Promise<never> {
  return failed;
}

export const elk = new ELK(
  __GRIBOVIK_EXPORT__ || typeof Worker === "undefined"
    ? {}
    : {
        workerFactory: () => {
          const worker = new Worker(
            new URL("elkjs/lib/elk-worker.min.js", import.meta.url),
            { type: "module" },
          );
          worker.addEventListener("error", (event) =>
            failWorker(new Error(`layout worker failed: ${event.message}`)),
          );
          return worker;
        },
      },
);
