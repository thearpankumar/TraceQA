// Lint for the TypeScript SDK, with type-aware rules. Run with `npm run lint`.
// The in-page collector has its own profile in eslint.collector.config.mjs.
import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**", "src/protocol.ts"] }, // protocol.ts is generated

  {
    files: ["src/**/*.ts", "test/**/*.ts"],
    extends: [js.configs.recommended, ...tseslint.configs.recommendedTypeChecked],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
  },

  {
    // node:test's describe() and it() return promises that the runner collects; awaiting them is wrong.
    files: ["test/**/*.ts"],
    rules: {
      "@typescript-eslint/no-floating-promises": "off",
      "@typescript-eslint/require-await": "off",
    },
  },
);
