const ASSET_MODULE_PATH = '/@waluau/assets.walu';

function resourceShape(type) {
  if (type === 'image') {
    return ['resources.ImageLoadResult', 'resources.ImageResource', 'resources.await_typed_image', 'resources.own_image'];
  }
  if (type === 'font') {
    return ['resources.FontLoadResult', 'resources.FontResource', 'resources.await_typed_font', 'resources.own_font'];
  }
  if (type === 'audio') {
    return ['resources.SoundLoadResult', 'resources.SoundResource', 'audio.await_typed_sound', 'resources.own_sound'];
  }
  throw new Error(`unsupported typed asset kind: ${type}`);
}

export function typedAssetModule(assetManifest) {
  if (assetManifest == null) return null;
  const assets = Object.entries(assetManifest)
    .filter(([, asset]) => asset.name != null);
  if (assets.length === 0) return null;

  const fields = assets.map(([, asset]) => {
    const [, resourceType] = resourceShape(asset.type);
    return `    ${asset.name}: ${resourceType}?`;
  });
  const initializers = assets.map(([, asset]) => `        ${asset.name} = nil,`);
  const loads = assets.map(([path, asset]) => {
    const [resultType, resourceType, awaitFunction, ownFunction] = resourceShape(asset.type);
    const args = asset.type === 'font'
      ? `${JSON.stringify(path)}, ${JSON.stringify(asset.family)}`
      : JSON.stringify(path);
    return `    local ${asset.name}_result: ${resultType} = ${awaitFunction}(${args})
    local maybe_${asset.name}: ${resourceType}? = ${asset.name}_result.resource
    if maybe_${asset.name} ~= nil then
        local value: ${resourceType} = maybe_${asset.name}::${resourceType}
        bundle.${asset.name} = value
        ${ownFunction}(owner, value)
    else
        local maybe_error: resources.ResourceError? = ${asset.name}_result.error
        if maybe_error ~= nil then
            table.insert(errors, maybe_error::resources.ResourceError)
        end
    end`;
  });

  return `-- Generated from the browser asset manifest. Do not edit.
local resources = require("waluau:engine/resources")
local audio = require("waluau:engine/audio")

export type Bundle = {
    owner: resources.Owner,
${fields.join(',\n')}
}
export type LoadResult = { bundle: Bundle, errors: {resources.ResourceError} }

function load(): LoadResult
    local owner: resources.Owner = resources.new_owner()
    local errors: {resources.ResourceError} = {}
    local bundle: Bundle = {
        owner = owner,
${initializers.join('\n')}
    }
${loads.join('\n')}
    return { bundle = bundle, errors = errors }
end

return { load = load }
`;
}

export function withTypedAssetModule(files, assetManifest) {
  const source = typedAssetModule(assetManifest);
  if (source == null) return files;
  return { ...files, [ASSET_MODULE_PATH]: source };
}
