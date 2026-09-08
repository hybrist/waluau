# City material atlas

`city-materials.png` is an albedo atlas used on real 3D geometry by
`src/city_scene_shaders.walu`. Its quadrants are slate, plaster, limestone and
wood, in reading order. Projection, silhouettes, depth, shadows and lighting
come from the scene geometry and shaders, rather than from this texture.

Generated with the built-in image-generation tool on 2026-09-08. The original
1254 × 1254 PNG is preserved without editing. Generation prompt:

> Production physically based architectural material texture atlas for a realistic medieval city rendered in real 3D. A square image EXACTLY divided into 2 columns and 2 rows of equal perfectly square quadrants, no borders, no gutters, no labels, no padding. Each quadrant is a flat orthographic texture photographed directly perpendicular to surface, evenly diffuse lit, no baked directional shadows or perspective. TOP LEFT: densely layered individual small weathered blue-grey slate roof tiles, subtle chipped irregular edges, natural mineral variation, fine moss in a few seams. TOP RIGHT: aged warm cream lime plaster with subtle stains, cracking, wear and small exposed patches, no windows or doors. BOTTOM LEFT: rough uneven medieval limestone ashlar masonry, organically irregular stone courses, varied buff grey stone, worn lime mortar, gritty and detailed. BOTTOM RIGHT: dark brown oak structural timber planks, rich long grain, hairline cracks, occasional small knots, natural weathered wood. Each of the four quadrants independently seamlessly tileable on both axes. High fidelity photorealistic material photography, rich fine detail and physically plausible restrained albedo. No objects, no buildings, no text, no illustrations, no raised isometric tiles, no black backgrounds. The whole canvas filled with the 4 material textures edge-to-edge.
