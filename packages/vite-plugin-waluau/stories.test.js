import assert from 'node:assert/strict';
import { test } from 'node:test';

import { exportNameFor, parseStories, parseStoryNames, storybookBinding } from './stories.js';

const source = (body) => `local storybook = require("waluau:engine/storybook")\n${body}\n`;

test('reads published story names in declaration order', () => {
  const names = parseStoryNames(source(`
storybook.publish({
    storybook.story("Face up", { draw = draw_face_up }),
    storybook.story("Face down", { draw = draw_face_down }),
    storybook.story("Every school", every_school_scene()),
})`));

  assert.deepEqual(names, ['Face up', 'Face down', 'Every school']);
});

test('follows the local name the story file binds the module to', () => {
  const book = `local book = require("waluau:engine/v1/storybook")
storybook.story("Not this one", {})
book.publish({ book.story("This one", {}) })`;

  assert.equal(storybookBinding(book), 'book');
  assert.deepEqual(parseStoryNames(book), ['This one']);
});

test('ignores files that do not require the storybook module', () => {
  assert.equal(storybookBinding('local engine = require("waluau:engine")'), null);
  assert.deepEqual(parseStoryNames('storybook.story("Orphan", {})'), []);
});

test('skips commented-out stories', () => {
  const names = parseStoryNames(source(`
-- storybook.story("Retired", {}),
--[[ storybook.story("Also retired", {}) ]]
storybook.story("Live", {})`));

  assert.deepEqual(names, ['Live']);
});

test('keeps escaped characters and drops repeated names', () => {
  const names = parseStoryNames(source(`
storybook.story("The \\"bound\\" state", {})
storybook.story('Single quoted', {})
storybook.story("Single quoted", {})`));

  assert.deepEqual(names, ['The "bound" state', 'Single quoted']);
});

test('derives unique JavaScript export names', () => {
  const taken = new Set();
  assert.equal(exportNameFor('Face up', taken), 'FaceUp');
  assert.equal(exportNameFor('face-up', taken), 'FaceUp2');
  assert.equal(exportNameFor('  the "bound" state ', taken), 'TheBoundState');
  assert.equal(exportNameFor('3 of a kind', taken), 'Story3OfAKind');
  assert.equal(exportNameFor('!!!', taken), 'Story');
});

test('pairs every story with the export it is published under', () => {
  const stories = parseStories(source(`
storybook.publish({
    storybook.story("Face up", {}),
    storybook.story("Face-up", {}),
})`));

  assert.deepEqual(stories, [
    { name: 'Face up', exportName: 'FaceUp' },
    { name: 'Face-up', exportName: 'FaceUp2' },
  ]);
});
