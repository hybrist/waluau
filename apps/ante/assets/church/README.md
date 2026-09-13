# Medieval church fidelity study

Open Ante Storybook with `pnpm storybook:ante`, then select **models / church**.
The four stories compare identical camera/light settings, inspect the overhead
footprint, switch a single LoD beside a two-story reference, and show small
map previews. Rotation, elevation, and zoom are manual controls; this study
does not pick an automatic distance threshold or change Ante's city map.

| Level | Description | GPU vertices | Triangles | Geometry bytes |
| --- | --- | ---: | ---: | ---: |
| 0 | Original beveled masonry and small details | 21,940 | 11,081 | 922,812 |
| 1 | Half the triangles, retains fine details | 15,233 | 5,539 | 614,856 |
| 2 | Reduced bevels, no small mullions or rose tracery | 7,118 | 3,663 | 300,204 |
| 3 | Main roof and wall silhouette, no windows or buttresses | 1,722 | 882 | 72,576 |

These are the **actual uploaded indexed vertices**, including splits for hard
normals and material colors. They are not Blender's welded vertex counts.
Geometry uses nine float32 values per vertex and uint32 triangle indices.
All colors are combined into one draw per church; shader/program overhead is
not included in the byte totals. Counts are budgets, not frame-rate measurements.

All levels use meters, the same ground origin, and a tower approximately
17.45 m tall. Simplification can move individual vertices by a few centimeters.
LOD 3 deliberately removes the entrance steps and small projections. The
cross-shaped nave/transept, apse, chapel roofs and tower remain recognizable.
This is a static visual asset assembled from overlapping mesh pieces.

## Authoring and regeneration

`source.blend` is the original authoring file. `church-lods.blend` has a separate
scene for each variant and a matched preview camera. The four GLBs contain
only the church mesh in their active scene; the original default cube is excluded.
The LoD generator uses Blender's mesh operations and runs as build tooling:

```sh
blender --background --factory-startup --python apps/ante/assets/church/generate_lods.py
pnpm --filter ante meshes:church
node --test tools/convert-static-mesh.test.mjs
```

On macOS the Blender executable is typically
`/Applications/Blender.app/Contents/MacOS/Blender`.
The generator exports GLBs and matching preview PNGs, as well as Blender counts.
`meshes:church` converts them into local generated `src/church/lod*.walu` modules
and records GPU counts in `gpu-lod-stats.json`. Both Storybook commands regenerate these modules before starting. Run
`meshes:church` after editing a GLB while the dev server is already running. They are compiled only by the church stories, so the game does
not load the study's assets.

The build-time converter accepts one uncompressed, identity-transform static
mesh with indexed triangles, positions, normals and opaque material colors.
Textures, animation, hierarchy, and material effects are outside this study's
supported import subset. It fails on unsupported data rather than silently
rendering an incorrect asset. Blender exports Y-up GLBs; the engine uses those
coordinates directly.

The engine renders with WebGL2 depth testing and a fixed directional light.
Blender preview PNGs use Workbench's cavity and shadow shading, so compare
fidelity in Storybook when judging the engine's final appearance.
