// Renderer half of the framework: the preview annotations that teach
// Storybook how to put a Waluau story on the canvas.
import { fileURLToPath } from 'node:url';

export const previewAnnotations = async (input = []) => [
  ...input,
  fileURLToPath(import.meta.resolve('@waluau/storybook/entry-preview')),
];
