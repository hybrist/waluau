// Build tooling for the deliberately small static-mesh subset used by Ante.
// No glTF parser ships in the browser. Fail loudly on unsupported authoring.
import { readFile, writeFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export function convertGlb(bytes) {
  const fail = message => { throw new Error(`Static mesh GLB: ${message}`); };
  if (bytes.length < 20 || bytes.readUInt32LE(0) !== 0x46546c67 || bytes.readUInt32LE(4) !== 2 || bytes.readUInt32LE(8) !== bytes.length) fail('invalid GLB header');
  let doc, binary;
  for (let offset = 12; offset < bytes.length;) {
    if (offset + 8 > bytes.length) fail('truncated chunk');
    const length = bytes.readUInt32LE(offset), type = bytes.readUInt32LE(offset + 4);
    offset += 8;
    if (offset + length > bytes.length) fail('chunk exceeds file');
    const chunk = bytes.subarray(offset, offset + length);
    if (type === 0x4e4f534a) doc = JSON.parse(chunk.toString('utf8'));
    if (type === 0x004e4942) binary = chunk;
    offset += length;
  }
  if (!doc || !binary || doc.extensionsRequired?.length || doc.animations?.length || doc.skins?.length) fail('requires an uncompressed static GLB');
  if (doc.meshes?.length !== 1) fail('export exactly one mesh');
  const nodes = doc.nodes?.filter(node => node.mesh !== undefined) ?? [];
  if (nodes.length !== 1 || nodes[0].mesh !== 0) fail('export exactly one mesh instance');
  const node = nodes[0];
  if (node.matrix || node.translation?.some(v => v !== 0) || node.rotation?.some((v, i) => v !== (i === 3 ? 1 : 0)) || node.scale?.some(v => v !== 1)) fail('apply object transforms before export');
  if (doc.nodes?.some(node => node.children?.length)) fail('flatten the scene before export');
  function accessor(id, expectedType, kinds) {
    const a = doc.accessors?.[id];
    if (!a || a.type !== expectedType || !kinds.includes(a.componentType) || a.sparse || a.normalized || !Number.isInteger(a.count) || a.count < 1) fail('unsupported accessor');
    const v = doc.bufferViews?.[a.bufferView];
    if (!v || v.buffer !== 0 || doc.buffers?.[0]?.uri) fail('accessor must use embedded buffer');
    const components = expectedType === 'VEC3' ? 3 : 1;
    const width = a.componentType === 5123 ? 2 : 4;
    const stride = v.byteStride ?? components * width;
    const start = (v.byteOffset ?? 0) + (a.byteOffset ?? 0);
    const end = start + (a.count - 1) * stride + components * width;
    if (stride < components * width || start < 0 || end > binary.length || end > (v.byteOffset ?? 0) + v.byteLength) fail('accessor out of bounds');
    const read = a.componentType === 5126 ? 'readFloatLE' : width === 2 ? 'readUInt16LE' : 'readUInt32LE';
    return Array.from({ length: a.count }, (_, i) => Array.from({ length: components }, (_, c) => binary[read](start + i * stride + c * width)));
  }
  const vertices = [], indices = [], unique = new Map();
  for (const primitive of doc.meshes[0].primitives) {
    if ((primitive.mode ?? 4) !== 4 || primitive.targets || primitive.extensions) fail('only plain triangles are supported');
    const material = doc.materials?.[primitive.material];
    if (!material || (material.alphaMode ?? 'OPAQUE') !== 'OPAQUE' || material.normalTexture || material.emissiveTexture || material.occlusionTexture || material.pbrMetallicRoughness?.baseColorTexture || material.pbrMetallicRoughness?.metallicRoughnessTexture) fail('only opaque material colors are supported');
    const color = material.pbrMetallicRoughness?.baseColorFactor ?? [1, 1, 1, 1];
    if (color[3] !== 1) fail('transparent material');
    const positions = accessor(primitive.attributes.POSITION, 'VEC3', [5126]);
    const normals = accessor(primitive.attributes.NORMAL, 'VEC3', [5126]);
    if (positions.length !== normals.length) fail('attribute counts disagree');
    const sourceIndices = accessor(primitive.indices, 'SCALAR', [5123, 5125]).flat();
    if (sourceIndices.length % 3) fail('incomplete triangle');
    for (const index of sourceIndices) {
      if (index >= positions.length) fail('index outside vertex array');
      const vertex = [...positions[index], ...normals[index], ...color.slice(0, 3)].map(Math.fround);
      if (!vertex.every(Number.isFinite)) fail('non-finite vertex');
      const key = vertex.join(',');
      if (!unique.has(key)) { unique.set(key, vertices.length / 9); vertices.push(...vertex); }
      indices.push(unique.get(key));
    }
  }
  const minimum = [Infinity, Infinity, Infinity], maximum = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < vertices.length; i += 9) for (let c = 0; c < 3; c++) {
    minimum[c] = Math.min(minimum[c], vertices[i + c]); maximum[c] = Math.max(maximum[c], vertices[i + c]);
  }
  return { vertices, indices, stats: { gpuVertices: vertices.length / 9, triangles: indices.length / 3, gpuBytes: vertices.length * 4 + indices.length * 4, minimum, maximum } };
}

export function waluauMesh({ vertices, indices }) {
  const array = (values, size) => Array.from({ length: Math.ceil(values.length / size) }, (_, i) => '        ' + values.slice(i * size, (i + 1) * size).map(v => Number(v.toPrecision(9))).join(', ')).join(',\n');
  return `-- Generated by tools/convert-static-mesh.mjs; edit the Blender source.\nlocal graphics = require("waluau:engine/graphics")\n\nfunction create(renderer: graphics.Graphics): graphics.StaticMesh\n    return renderer:create_static_mesh(\n        {\n${array(vertices, 9)}\n        }::Float32Array,\n        {\n${array(indices, 18)}\n        }::Uint32Array\n    )\nend\n\nreturn { create = create }\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , input, output] = process.argv;
  if (!input || !output) throw new Error('Usage: node tools/convert-static-mesh.mjs input.glb output.walu');
  const mesh = convertGlb(await readFile(input));
  await writeFile(output, waluauMesh(mesh));
  console.log(JSON.stringify(mesh.stats, null, 2));
}
