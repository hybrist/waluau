#!/usr/bin/env node
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import idl from '@webref/idl';
import { parse as parseWebIdl } from 'webidl2';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

const defaults = {
  input: 'tools/dom-idl/input/custom.webidl',
  filter: 'tools/dom-idl/filter.json',
  patches: 'tools/dom-idl/patches.json',
  out: 'externs/dom.walu',
  metadataOut: 'externs/dom.metadata.json',
  diagnosticsOut: 'externs/dom.diagnostics.txt',
};

function parseArgs(argv) {
  const options = { ...defaults };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith('--')) {
      throw new Error(`unexpected argument: ${arg}`);
    }
    const key = arg.slice(2).replaceAll('-', '');
    const value = argv[i + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`missing value for ${arg}`);
    }
    i += 1;
    if (key === 'metadataout') options.metadataOut = value;
    else if (key === 'diagnosticsout') options.diagnosticsOut = value;
    else if (Object.hasOwn(options, key)) options[key] = value;
    else throw new Error(`unknown option: ${arg}`);
  }
  return options;
}

function resolveRepoPath(value) {
  return path.resolve(repoRoot, value);
}

function memberKey(member) {
  return `${member.type}:${member.name || ''}`;
}

function stringifyType(node) {
  if (!node) {
    throw new Error('stringifyType called with null/undefined node');
  }
  if (node.union) {
    const members = node.idlType.map(stringifyType).join(' or ');
    return node.nullable ? `(${members})?` : `(${members})`;
  }
  if (node.generic) {
    const parameters = node.idlType.map(stringifyType).join(', ');
    const base = `${node.generic}<${parameters}>`;
    return node.nullable ? `${base}?` : base;
  }
  const base = node.idlType;
  return node.nullable ? `${base}?` : base;
}

function convertAstMember(member) {
  if (member.type === 'attribute') {
    if (!member.idlType) return null;
    return {
      kind: 'attribute',
      readonly: member.readonly,
      idlType: stringifyType(member.idlType),
      name: member.name,
      source: member.type,
    };
  }

  if (member.type === 'operation') {
    if (!member.idlType) return null;
    const params = [];
    for (const arg of member.arguments) {
      if (arg.optional) {
        break;
      }
      if (!arg.idlType) {
        return null;
      }
      params.push({
        optional: false,
        idlType: stringifyType(arg.idlType),
        name: arg.name,
        source: arg.type,
      });
    }

    return {
      kind: 'operation',
      idlType: stringifyType(member.idlType),
      name: member.name,
      params,
      source: member.type,
    };
  }

  return null;
}

async function parseAndMergeIdls(customSource) {
  const allSpecs = await idl.parseAll();
  const interfaces = new Map();
  const mixins = new Map();
  const includes = [];

  // 1. Process standard Web IDL specs
  for (const [spec, ast] of Object.entries(allSpecs)) {
    for (const def of ast) {
      processDef(def);
    }
  }

  // 2. Process custom Web IDL spec
  if (customSource) {
    const customAst = parseWebIdl(customSource);
    for (const def of customAst) {
      processDef(def);
    }
  }

  function processDef(def) {
    if (def.type === 'interface') {
      if (!interfaces.has(def.name)) {
        interfaces.set(def.name, {
          name: def.name,
          type: 'interface',
          partials: [],
          members: []
        });
      }
      const entry = interfaces.get(def.name);
      if (def.partial) {
        entry.partials.push(def);
      } else {
        entry.main = def;
        entry.parent = def.inheritance;
      }
    } else if (def.type === 'interface mixin') {
      if (!mixins.has(def.name)) {
        mixins.set(def.name, {
          name: def.name,
          type: 'interface mixin',
          partials: [],
          members: []
        });
      }
      const entry = mixins.get(def.name);
      if (def.partial) {
        entry.partials.push(def);
      } else {
        entry.main = def;
      }
    } else if (def.type === 'includes') {
      includes.push(def);
    }
  }

  // 3. Resolve members
  const mergedInterfaces = new Map();

  for (const [name, info] of interfaces.entries()) {
    const membersMap = new Map();

    if (info.main) {
      for (const member of info.main.members) {
        const key = memberKey(member);
        if (key) membersMap.set(key, member);
      }
    }

    for (const partial of info.partials) {
      for (const member of partial.members) {
        const key = memberKey(member);
        if (key) membersMap.set(key, member);
      }
    }

    mergedInterfaces.set(name, {
      name,
      parent: info.parent || null,
      membersMap,
    });
  }

  const mergedMixins = new Map();
  for (const [name, info] of mixins.entries()) {
    const membersMap = new Map();
    if (info.main) {
      for (const member of info.main.members) {
        const key = memberKey(member);
        if (key) membersMap.set(key, member);
      }
    }
    for (const partial of info.partials) {
      for (const member of partial.members) {
        const key = memberKey(member);
        if (key) membersMap.set(key, member);
      }
    }
    mergedMixins.set(name, { name, membersMap });
  }

  // Apply includes
  for (const inc of includes) {
    const target = mergedInterfaces.get(inc.target);
    const mixin = mergedMixins.get(inc.includes);
    if (target && mixin) {
      for (const [key, member] of mixin.membersMap.entries()) {
        if (!target.membersMap.has(key)) {
          target.membersMap.set(key, member);
        }
      }
    }
  }

  // Convert to simplified structures expected by generator
  const result = [];
  for (const [name, info] of mergedInterfaces.entries()) {
    const members = [];
    for (const astMember of info.membersMap.values()) {
      const converted = convertAstMember(astMember);
      if (converted) {
        members.push(converted);
      }
    }
    result.push({
      name,
      parent: info.parent,
      members,
    });
  }

  return result;
}

function mapType(idlType, filter, knownInterfaces) {
  const nullable = idlType.endsWith('?');
  const base = nullable ? idlType.slice(0, -1).trim() : idlType;
  if (/^(sequence|FrozenArray|Promise)\s*</.test(base)) {
    return { error: `unsupported generic Web IDL type ${idlType}` };
  }
  const mapped = filter.typeMap[base] ?? (knownInterfaces.has(base) ? base : null);
  if (!mapped) {
    return { error: `unsupported Web IDL type ${idlType}` };
  }
  return { type: nullable ? `${mapped}?` : mapped };
}

function patchedName(name, patches) {
  return patches.memberRenames?.[name] ?? name;
}

function patchedParamName(owner, member, param, patches) {
  return patches.parameterRenames?.[`${owner}.${member}.${param}`] ?? toSnake(param);
}

function toSnake(name) {
  return name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
}

function emitInterfaceMember(iface, member, context) {
  const { filter, patches, knownInterfaces } = context;
  const rename = patchedName(member.name, patches);

  if (member.kind === 'attribute') {
    const mapped = mapType(member.idlType, filter, knownInterfaces);
    if (mapped.error) return { skipped: mapped.error };
    return {
      line: `declare property ${iface.name}:${rename}: ${mapped.type}`,
      metadata: {
        interface: iface.name,
        kind: member.kind,
        idlName: member.name,
        emittedName: rename,
        type: mapped.type,
        readonly: member.readonly,
      },
    };
  }

  if (member.kind === 'operation') {
    const returnType = mapType(member.idlType, filter, knownInterfaces);
    if (returnType.error) return { skipped: returnType.error };
    const params = [];
    for (const param of member.params) {
      if (param.unsupported) return { skipped: `unsupported parameter syntax ${param.source}` };
      if (param.optional) return { skipped: `unsupported optional parameter ${param.name}` };
      const mapped = mapType(param.idlType, filter, knownInterfaces);
      if (mapped.error) return { skipped: `${param.name}: ${mapped.error}` };
      params.push(`${patchedParamName(iface.name, member.name, param.name, patches)}: ${mapped.type}`);
    }
    return {
      line: `declare function ${iface.name}:${rename}(${params.join(', ')}): ${returnType.type}`,
      metadata: {
        interface: iface.name,
        kind: member.kind,
        idlName: member.name,
        emittedName: rename,
        params,
        returnType: returnType.type,
      },
    };
  }

  return { skipped: 'unsupported member syntax' };
}

function emitExternTypeLine(iface, include) {
  if (iface.parent && include.has(iface.parent)) {
    return `type ${iface.name} = extern extends ${iface.parent}`;
  }
  return `type ${iface.name} = extern`;
}

async function generate({ customSource, filter, patches }) {
  const parsed = await parseAndMergeIdls(customSource);
  const include = new Set(filter.interfaces);
  const knownInterfaces = new Set(parsed.map((iface) => iface.name));
  const diagnostics = [];
  const emittedMembers = [];
  const output = [
    '-- Generated by tools/dom-idl/generate-dom-externs.mjs; DO NOT EDIT.',
    '-- Source: @webref/idl (W3C Webref)',
    '-- Inheritance is emitted with extern extends and mirrored in externs/dom.metadata.json.',
    '',
  ];
  const metadata = {
    source: '@webref/idl',
    inheritance: [],
    emittedMembers,
    emittedHostFunctions: [],
    skippedMembers: [],
  };

  for (const iface of parsed) {
    if (!include.has(iface.name)) {
      continue;
    }
    metadata.inheritance.push({ interface: iface.name, parent: iface.parent });
  }

  const interfacesByName = new Map(parsed.map((iface) => [iface.name, iface]));
  for (const name of filter.interfaces) {
    const iface = interfacesByName.get(name);
    if (!iface) {
      diagnostics.push(`skip interface ${name}: selected by filter but missing from IDL`);
      continue;
    }
    if (iface.parent && !include.has(iface.parent)) {
      diagnostics.push(`skip inheritance ${iface.name} -> ${iface.parent}: parent not selected by filter`);
    }
    output.push(emitExternTypeLine(iface, include));
  }
  output.push('');

  for (const iface of parsed.filter((candidate) => include.has(candidate.name))) {
    const selectedMembers = new Set(filter.members[iface.name] ?? []);
    for (const member of iface.members) {
      const key = `${member.kind}:${member.name}`;
      if (!selectedMembers.has(key)) {
        continue;
      }
      const emitted = emitInterfaceMember(iface, member, { filter, patches, knownInterfaces });
      if (emitted.skipped) {
        const diagnostic = `skip ${iface.name}.${member.name ?? '<anonymous>'}: ${emitted.skipped}`;
        diagnostics.push(diagnostic);
        metadata.skippedMembers.push({ interface: iface.name, member: member.name ?? null, reason: emitted.skipped });
        continue;
      }
      output.push(emitted.line);
      emittedMembers.push(emitted.metadata);
    }
  }

  for (const hostFunction of filter.hostFunctions ?? []) {
    const returnType = mapType(hostFunction.returnType, filter, knownInterfaces);
    if (returnType.error) {
      diagnostics.push(`skip host function ${hostFunction.name}: ${returnType.error}`);
      continue;
    }
    output.push(`declare function ${hostFunction.name}(): ${returnType.type}`);
    metadata.emittedHostFunctions.push({
      name: hostFunction.name,
      returnType: returnType.type,
    });
  }

  output.push('');
  return {
    externs: output.join('\n'),
    metadata: `${JSON.stringify(metadata, null, 2)}\n`,
    diagnostics: `${diagnostics.sort().join('\n')}\n`,
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  let customSource = null;
  try {
    customSource = await readFile(resolveRepoPath(options.input), 'utf8');
  } catch (err) {
    if (err.code !== 'ENOENT') {
      throw err;
    }
  }
  const [filterRaw, patchesRaw] = await Promise.all([
    readFile(resolveRepoPath(options.filter), 'utf8'),
    readFile(resolveRepoPath(options.patches), 'utf8'),
  ]);
  const generated = await generate({
    customSource,
    filter: JSON.parse(filterRaw),
    patches: JSON.parse(patchesRaw),
  });
  for (const target of [options.out, options.metadataOut, options.diagnosticsOut]) {
    await mkdir(path.dirname(resolveRepoPath(target)), { recursive: true });
  }
  await Promise.all([
    writeFile(resolveRepoPath(options.out), generated.externs),
    writeFile(resolveRepoPath(options.metadataOut), generated.metadata),
    writeFile(resolveRepoPath(options.diagnosticsOut), generated.diagnostics),
  ]);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

export { generate, parseAndMergeIdls };
