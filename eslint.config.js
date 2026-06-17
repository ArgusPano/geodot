import js from "@eslint/js";

export default [
  js.configs.recommended,
  {
    files: ["js/**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      globals: {
        Buffer: "readonly",
        console: "readonly",
        fetch: "readonly",
        globalThis: "readonly",
        performance: "readonly",
        process: "readonly",
        Response: "readonly",
      },
    },
  },
];
