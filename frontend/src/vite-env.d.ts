/** Markdown files imported as raw strings (docs/legal). */
declare module "*.md?raw" {
  const content: string;
  export default content;
}
