import { existsSync } from "node:fs";

export const TerraneCargoCache = async () => {
  return {
    "shell.env": async (_input, output) => {
      const home = process.env.HOME;
      if (!home) return;

      output.env.CARGO_TARGET_DIR ??=
        `${home}/Library/Caches/terrane/cargo-target/all`;
      output.env.SCCACHE_DIR ??= `${home}/Library/Caches/sccache`;
      output.env.SCCACHE_CACHE_SIZE ??= "40G";

      if (!output.env.RUSTC_WRAPPER) {
        for (
          const candidate of [
            "/opt/homebrew/bin/sccache",
            "/usr/local/bin/sccache",
          ]
        ) {
          if (existsSync(candidate)) {
            output.env.RUSTC_WRAPPER = candidate;
            return;
          }
        }
        output.env.RUSTC_WRAPPER = "sccache";
      }
    },
  };
};
