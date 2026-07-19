# @waluau/vite-plugin

This plugin turns a Vite app into a Waluau game host. It compiles
`src/main.walu`, injects the generated game entry into `index.html`, supplies
the standard browser imports, and lets the game own the entire viewport by
default.

```js
// vite.config.js
import { defineConfig } from 'vite';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau()],
});
```

The default project needs only an `index.html` and `src/main.walu`. Set
`entry` or `fullScreen` to override the defaults. Outside the Waluau
repository, install the `waluau` compiler binary or pass a custom `compiler`
command.
