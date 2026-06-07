#!/usr/bin/env node
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

const defaults = {
  input: 'tools/dom-idl/input/dom-first-slice.webidl',
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

function stripComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
}

function splitMembers(body) {
  return body
    .split(';')
    .map((member) => member.trim())
    .filter(Boolean)
    .map((member) => member.replace(/^\[[^\]]+\]\s*/g, '').trim());
}

function parseIdl(source) {
  const interfaces = [];
  const clean = stripComments(source);
  const interfacePattern = /interface\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*:\s*([A-Za-z_][A-Za-z0-9_]*))?\s*\{([\s\S]*?)\};/g;
  let match;
  while ((match = interfacePattern.exec(clean)) !== null) {
    const [, name, parent = null, body] = match;
    const members = splitMembers(body).map((member) => parseMember(member));
    interfaces.push({ name, parent, members });
  }
  return interfaces;
}

function parseMember(member) {
  const attribute = /^(readonly\s+)?attribute\s+(.+?)\s+([A-Za-z_][A-Za-z0-9_]*)$/.exec(member);
  if (attribute) {
    return {
      kind: 'attribute',
      readonly: Boolean(attribute[1]),
      idlType: normalizeIdlType(attribute[2]),
      name: attribute[3],
      source: member,
    };
  }

  const operation = /^(.+?)\s+([A-Za-z_][A-Za-z0-9_]*)\((.*)\)$/.exec(member);
  if (operation) {
    return {
      kind: 'operation',
      idlType: normalizeIdlType(operation[1]),
      name: operation[2],
      params: parseParams(operation[3]),
      source: member,
    };
  }

  return { kind: 'unsupported', source: member };
}

function parseParams(source) {
  const trimmed = source.trim();
  if (!trimmed) return [];
  return trimmed.split(',').map((param) => {
    const parts = param.trim().split(/\s+/);
    const optional = parts[0] === 'optional';
    const useful = optional ? parts.slice(1) : parts;
    if (useful.length < 2) {
      return { unsupported: true, source: param.trim() };
    }
    return {
      optional,
      idlType: normalizeIdlType(useful.slice(0, -1).join(' ')),
      name: useful[useful.length - 1],
      source: param.trim(),
    };
  });
}

function normalizeIdlType(idlType) {
  return idlType.replace(/\s+/g, ' ').trim();
}

function memberKey(member) {
  return `${member.kind}:${member.name}`;
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

function generate({ idlSource, filter, patches }) {
  const parsed = parseIdl(idlSource);
  const include = new Set(filter.interfaces);
  const knownInterfaces = new Set(parsed.map((iface) => iface.name));
  const diagnostics = [];
  const emittedMembers = [];
  const output = [
    '-- Generated by tools/dom-idl/generate-dom-externs.mjs; DO NOT EDIT.',
    '-- Source: tools/dom-idl/input/dom-first-slice.webidl',
    '-- Inheritance is emitted with extern extends and mirrored in externs/dom.metadata.json.',
    '',
  ];
  const metadata = {
    source: defaults.input,
    inheritance: [],
    emittedMembers,
    emittedHostFunctions: [],
    skippedMembers: [],
  };

  for (const iface of parsed) {
    if (!include.has(iface.name)) {
      diagnostics.push(`skip interface ${iface.name}: not selected by filter`);
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
      if (!selectedMembers.has(memberKey(member))) {
        diagnostics.push(`skip ${iface.name}.${member.name ?? '<anonymous>'}: not selected by filter`);
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
  const [idlSource, filterRaw, patchesRaw] = await Promise.all([
    readFile(resolveRepoPath(options.input), 'utf8'),
    readFile(resolveRepoPath(options.filter), 'utf8'),
    readFile(resolveRepoPath(options.patches), 'utf8'),
  ]);
  const generated = generate({
    idlSource,
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
    console.error(error.message);
    process.exitCode = 1;
  });
}

export { generate, parseIdl };
