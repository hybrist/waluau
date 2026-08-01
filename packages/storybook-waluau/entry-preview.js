// The Waluau renderer, loaded into Storybook's preview. A story's result is
// its book plus the name the book published it under; putting it on screen is
// asking that book to mount it into the element Storybook is showing.

export const parameters = { renderer: 'waluau' };

/** Generated story render functions pair Storybook args with their Waluau book. */
export const render = (args, context) => {
  const { component } = context;
  if (typeof component === 'function') return component(args, context);
  throw new Error(
    `The Waluau story "${context.name}" has no render function. Stories are generated from a `
    + '.stories.walu file; a hand-written CSF module needs its own render().',
  );
};

export async function renderToCanvas({ storyFn, name, showMain, showError }, canvasElement) {
  const result = storyFn();
  if (result == null || typeof result.book?.mount !== 'function') {
    showError({
      title: `Expecting a Waluau story from "${name}".`,
      description:
        'Stories come from a .stories.walu file, where each one is declared with '
        + 'storybook.story("<name>", { draw = ... }) and published with storybook.publish.',
    });
    return undefined;
  }

  await result.book.mount(result.name, canvasElement, result.args);
  showMain();
  // Storybook replaces the canvas element between stories from different
  // files; tearing the session down here is what stops the old animation loop
  // and releases its WebGL context.
  return () => result.book.unmount();
}
