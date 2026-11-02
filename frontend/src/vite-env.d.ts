// The project has no `@types/node`, so a test that greps a source file reads it with
// Vite's `?raw` import instead of `node:fs`. This is the only declaration Vite needs.
declare module "*?raw" {
  const content: string;
  export default content;
}
