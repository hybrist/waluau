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

function mapType(idlType, filter, knownInterfaces, include) {
  const nullable = idlType.endsWith('?');
  const base = nullable ? idlType.slice(0, -1).trim() : idlType;
  const genericMatch = base.match(/^(sequence|FrozenArray|Promise|ObservableArray|record)\s*</);
  if (genericMatch) {
    return {
      error: `unsupported generic Web IDL type ${idlType}`,
      category: `unsupported-generic:${genericMatch[1]}`,
    };
  }
  if (base.startsWith('(')) {
    return { error: `unsupported union Web IDL type ${idlType}`, category: 'unsupported-union' };
  }
  // waluau-lxdd: the extern type system has no `any`/`object`/dynamic value type, so
  // Web IDL 'any' and 'object' (e.g. OnErrorEventHandler returns, postMessage payloads,
  // CustomElementConstructor) have no signature representation.
  if (base === 'any' || base === 'object') {
    return { error: `unsupported Web IDL type ${idlType}`, category: 'untyped-dynamic-type' };
  }
  if (filter.typeMap[base]) {
    const mapped = filter.typeMap[base];
    return finishMapType(idlType, mapped, nullable);
  }
  // An interface name is only a valid extern type reference if it's part of the selected
  // surface -- referencing a known-but-unselected IDL interface (e.g. ReadableStream) would
  // emit a type name with no matching `type X = extern` declaration, producing externs that
  // fail to compile ("unknown type 'X'").
  if (include.has(base)) {
    return finishMapType(idlType, base, nullable);
  }
  if (knownInterfaces.has(base)) {
    return { error: `Web IDL type ${idlType} is not part of the selected DOM interface surface`, category: 'out-of-surface-type' };
  }
  return { error: `unsupported Web IDL type ${idlType}`, category: 'unrepresented-type' };
}

function finishMapType(idlType, mapped, nullable) {
  if (nullable) {
    // waluau-lqjs: '?' has no syntax for function/callback types (EventListener?, EventHandler?)
    if (mapped.includes('->')) {
      return {
        error: `nullable callback Web IDL type ${idlType} has no extern syntax representation`,
        category: 'nullable-callback-type',
      };
    }
    // waluau-pq7p: '?' is only supported on host reference types (string, extern); the type
    // checker rejects nullable numerics/bool/unit (e.g. unsigned long?, double?, boolean?)
    if (NULLABLE_REJECTED_TYPES.has(mapped)) {
      return {
        error: `nullable modifier rejects primitive Web IDL type ${idlType}`,
        category: 'nullable-primitive-type',
      };
    }
  }
  return { type: nullable ? `${mapped}?` : mapped };
}

const NULLABLE_REJECTED_TYPES = new Set(['u32', 'u64', 'i32', 'i64', 'f32', 'f64', 'bool', 'unit']);

// Mirrors crates/waluau-lexer/src/lib.rs keyword table. Generated extern parameter
// names are snake_cased from the IDL name; if the result collides with a reserved
// word, the whole externs file fails to parse (waluau-9szs).
const RESERVED_WORDS = new Set([
  'fn', 'let', 'function', 'local', 'if', 'then', 'elseif', 'else', 'end', 'while', 'for', 'in',
  'repeat', 'until', 'do', 'return', 'break', 'continue', 'not', 'and', 'or',
  'number', 'u32', 'u64', 'i32', 'i64', 'f32', 'f64', 'unit', 'void', 'bool', 'unknown', 'string',
  'bytes', 'extern', 'thread', 'nil', 'true', 'false',
]);

function patchedName(name, patches) {
  return patches.memberRenames?.[name] ?? name;
}

function patchedParamName(owner, member, param, patches) {
  return patches.parameterRenames?.[`${owner}.${member}.${param}`] ?? toSnake(param);
}

function toSnake(name) {
  return name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
}

function sanitizeParamName(name, usedNames) {
  let candidate = name;
  while (RESERVED_WORDS.has(candidate) || usedNames.has(candidate)) {
    candidate = `${candidate}_`;
  }
  usedNames.add(candidate);
  return candidate;
}

function emitInterfaceMember(iface, member, context) {
  const { filter, patches, knownInterfaces, include } = context;
  const rename = patchedName(member.name, patches);

  if (member.kind === 'attribute') {
    const mapped = mapType(member.idlType, filter, knownInterfaces, include);
    if (mapped.error) return { skipped: mapped.error, category: mapped.category };
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
    const returnType = mapType(member.idlType, filter, knownInterfaces, include);
    if (returnType.error) return { skipped: returnType.error, category: returnType.category };
    const params = [];
    const usedParamNames = new Set();
    for (const param of member.params) {
      if (param.unsupported) return { skipped: `unsupported parameter syntax ${param.source}` };
      if (param.optional) return { skipped: `unsupported optional parameter ${param.name}` };
      const mapped = mapType(param.idlType, filter, knownInterfaces, include);
      if (mapped.error) return { skipped: `${param.name}: ${mapped.error}`, category: mapped.category };
      const paramName = sanitizeParamName(
        patchedParamName(iface.name, member.name, param.name, patches),
        usedParamNames,
      );
      params.push(`${paramName}: ${mapped.type}`);
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

function sortSelectedInterfaceNames(selectedNames, interfacesByName, include) {
  const ordered = [];
  const permanent = new Set();
  const visiting = new Set();

  function visit(name) {
    if (permanent.has(name)) {
      return;
    }
    const iface = interfacesByName.get(name);
    if (!iface) {
      return;
    }
    if (visiting.has(name)) {
      throw new Error(`cyclic DOM inheritance detected for ${name}`);
    }

    visiting.add(name);
    if (iface.parent && include.has(iface.parent)) {
      visit(iface.parent);
    }
    visiting.delete(name);
    permanent.add(name);
    ordered.push(name);
  }

  for (const name of selectedNames) {
    visit(name);
  }

  return ordered;
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

  const interfacesByName = new Map(parsed.map((iface) => [iface.name, iface]));
  const selectedInterfaceNames = sortSelectedInterfaceNames(filter.interfaces, interfacesByName, include);

  for (const name of filter.interfaces) {
    if (!interfacesByName.has(name)) {
      diagnostics.push(`skip interface ${name}: selected by filter but missing from IDL`);
    }
  }

  for (const name of selectedInterfaceNames) {
    const iface = interfacesByName.get(name);
    if (iface.parent && !include.has(iface.parent)) {
      diagnostics.push(`skip inheritance ${iface.name} -> ${iface.parent}: parent not selected by filter`);
    }
    metadata.inheritance.push({ interface: iface.name, parent: iface.parent });
    output.push(emitExternTypeLine(iface, include));
  }
  output.push('');

  // Negative filter: every member of a selected interface is emitted by default.
  // filter.disabledMembers documents the (interface, member) pairs that are held back,
  // each with a human-readable reason and -- where the cause is a tracked compiler/tooling
  // limitation rather than a deliberate scoping choice -- a beads issue reference.
  for (const iface of parsed.filter((candidate) => include.has(candidate.name))) {
    const disabledMembers = new Map((filter.disabledMembers?.[iface.name] ?? []).map((entry) => [entry.member, entry]));
    const seen = new Set();
    for (const member of iface.members) {
      if (!member.name) continue;
      const key = `${member.kind}:${member.name}`;
      if (seen.has(key)) continue;
      seen.add(key);

      const disabledEntry = disabledMembers.get(key);
      if (disabledEntry) {
        const suffix = disabledEntry.issue ? ` (${disabledEntry.issue})` : '';
        diagnostics.push(`skip ${iface.name}.${member.name}: disabled -- ${disabledEntry.reason}${suffix}`);
        metadata.skippedMembers.push({
          interface: iface.name,
          member: member.name,
          reason: disabledEntry.reason,
          issue: disabledEntry.issue ?? null,
          disabled: true,
        });
        continue;
      }

      const emitted = emitInterfaceMember(iface, member, { filter, patches, knownInterfaces, include });
      if (emitted.skipped) {
        const issue = filter.skipIssueRefs?.[emitted.category] ?? null;
        const suffix = issue ? ` (${issue})` : '';
        diagnostics.push(`skip ${iface.name}.${member.name ?? '<anonymous>'}: ${emitted.skipped}${suffix}`);
        metadata.skippedMembers.push({
          interface: iface.name,
          member: member.name ?? null,
          reason: emitted.skipped,
          issue,
        });
        continue;
      }
      output.push(emitted.line);
      emittedMembers.push(emitted.metadata);
    }
  }

  for (const hostFunction of filter.hostFunctions ?? []) {
    const returnType = mapType(hostFunction.returnType, filter, knownInterfaces, include);
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
