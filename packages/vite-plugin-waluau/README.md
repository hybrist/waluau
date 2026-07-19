# @waluau/vite-plugin

This plugin makes `.walu` files ordinary Vite modules. Importing one compiles
the Waluau project rooted at that file, supplies the standard browser imports,
starts the game, and exports its loading promise as both `game` and the default
export. The generated game owns the entire viewport by default.

```js
// vite.config.js
import { defineConfig } from 'vite';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau()],
});
```

Point a normal module script at the Waluau source:

```html
<script type="module" src="/src/main.walu"></script>
```

The same file can be imported from JavaScript with
`import game from './main.walu'`. Set `fullScreen: false` to embed the game
without the viewport styles. Outside the Waluau repository, install the
`waluau` compiler binary or pass a custom `compiler` command.
