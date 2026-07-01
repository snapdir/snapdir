// eslint.config.js — flat config for @snapdir/snapdir Node binding (ESLint 9)
'use strict'

const tsParser = require('@typescript-eslint/parser')
const tsPlugin = require('@typescript-eslint/eslint-plugin')

/** @type {import('eslint').Linter.Config[]} */
module.exports = [
  // Ignore generated and vendored files
  {
    ignores: [
      'node_modules/**',
      'dist/**',
      'index.js',
      'index.mjs',
      'index.d.ts',
      '*.node',
    ],
  },

  // TypeScript test files
  {
    files: ['test/**/*.ts'],
    plugins: {
      '@typescript-eslint': tsPlugin,
    },
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        project: './tsconfig.json',
        tsconfigRootDir: __dirname,
        ecmaVersion: 2022,
        sourceType: 'module',
      },
      globals: {
        process: 'readonly',
        __dirname: 'readonly',
        __filename: 'readonly',
        require: 'readonly',
        module: 'readonly',
        exports: 'readonly',
        console: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        setInterval: 'readonly',
        clearInterval: 'readonly',
        Promise: 'readonly',
        Buffer: 'readonly',
      },
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      // Allow @ts-expect-error and @ts-ignore in adversary-authored test files
      '@typescript-eslint/ban-ts-comment': 'off',
      // Allow explicit `any` in test/spec files where the binding surface is not yet typed
      '@typescript-eslint/no-explicit-any': 'off',
      // Allow unused vars prefixed with _ (common in spec stubs)
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      // Allow empty functions in test stubs
      '@typescript-eslint/no-empty-function': 'off',
    },
  },

  // CJS scripts — plain JS
  {
    files: ['scripts/**/*.cjs'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'commonjs',
      globals: {
        process: 'readonly',
        __dirname: 'readonly',
        __filename: 'readonly',
        require: 'readonly',
        module: 'readonly',
        exports: 'readonly',
        console: 'readonly',
      },
    },
  },
]
