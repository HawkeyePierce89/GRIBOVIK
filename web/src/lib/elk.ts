import ELK from "elkjs/lib/elk.bundled.js";

let failed: Promise<never> = new Promise(() => {});

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
          failed = new Promise((_, reject) => {
            worker.addEventListener("error", (event) =>
              reject(new Error(`layout worker failed: ${event.message}`)),
            );
          });
          return worker;
        },
      },
);
