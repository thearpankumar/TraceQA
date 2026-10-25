// Lint profile for the in-page collector (page-script/page-script.js and pristine.js). Run with
// `npm run lint:collector`, which starts from the repository root so the files are inside ESLint's base path.
import js from "@eslint/js";

// The collector runs unchanged in every browser a driver can drive, and the engine ships it as a
// string. It is written as plain ES5: no let/const, arrow functions or template literals.
const es5Syntax = [
  { selector: "VariableDeclaration[kind='let']", message: "The collector is ES5: use var." },
  { selector: "VariableDeclaration[kind='const']", message: "The collector is ES5: use var." },
  { selector: "ArrowFunctionExpression", message: "The collector is ES5: use a function expression." },
  { selector: "TemplateLiteral", message: "The collector is ES5: concatenate strings." },
  { selector: "SpreadElement", message: "The collector is ES5: no spread." },
  { selector: "ClassDeclaration", message: "The collector is ES5: no classes." },
];

export default [
  {
    files: ["page-script/page-script.js", "page-script/pristine.js"],
    languageOptions: {
      ecmaVersion: 5,
      sourceType: "script",
      globals: {
        window: "readonly",
        document: "readonly",
        location: "readonly",
        CSS: "readonly",
        WeakMap: "readonly",
        Set: "readonly",
        self: "readonly",
      },
    },
    rules: {
      ...js.configs.recommended.rules,
      "no-undef": "error",
      // ES5 has no `catch {}`: a handler that ignores the error still has to name it.
      "no-unused-vars": ["error", { caughtErrors: "none" }],
      "no-restricted-syntax": ["error", ...es5Syntax],
    },
  },
];
