"""Run in Blender: blender --background --python generate_lods.py.

The saved source is untouched. Every level shares the original ground origin,
meters, materials and tower finial. Output scenes are independent for exporting.
"""
from pathlib import Path
import bpy
import bmesh
import json
import math

ROOT = Path(__file__).resolve().parent
with bpy.data.libraries.load(str(ROOT / 'source.blend'), link=False) as (source, target):
    target.objects = [name for name in source.objects if name.startswith('Medieval Church |')]
original = target.objects[0]
assert original is not None


def components(bm):
    pending = set(bm.verts)
    while pending:
        stack = [pending.pop()]
        group = []
        while stack:
            vertex = stack.pop()
            group.append(vertex)
            for edge in vertex.link_edges:
                neighbor = edge.other_vert(vertex)
                if neighbor in pending:
                    pending.remove(neighbor)
                    stack.append(neighbor)
        yield group


def simplify(obj, level):
    if level == 0:
        return
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    for group in list(components(bm)):
        low = [min(v.co[i] for v in group) for i in range(3)]
        high = [max(v.co[i] for v in group) for i in range(3)]
        size = [high[i] - low[i] for i in range(3)]
        faces = {face for vertex in group for face in vertex.link_faces}
        materials = {obj.data.materials[face.material_index].name for face in faces}
        finial = low[2] > 16.3
        eastern_chapel = low[1] > 10.0
        roof = any('slate' in name.lower() or 'copper' in name.lower() for name in materials)
        discard = (level == 2 and max(size) < 1.05) or (level == 3 and max(size) < 4.9 and not roof and not eastern_chapel)
        if discard and not finial:
            bmesh.ops.delete(bm, geom=group, context='VERTS')
            continue
        # Remove small bevels within each masonry piece, preserving the separate
        # layers of glass, carved surrounds and door overlays.
        if level >= 2 and len(group) >= 20 and min(size) > .15:
            bmesh.ops.remove_doubles(bm, verts=group, dist=.12)
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.to_mesh(obj.data)
    bm.free()
    if level == 1:
        mod = obj.modifiers.new('Half detail', 'DECIMATE')
        mod.ratio = .5
        mod.use_collapse_triangulate = True
        bpy.ops.object.modifier_apply(modifier=mod.name)
    elif level == 3:
        mod = obj.modifiers.new('Planar structural faces', 'DECIMATE')
        mod.decimate_type = 'DISSOLVE'
        mod.angle_limit = math.radians(3)
        mod.delimit = {'MATERIAL'}
        bpy.ops.object.modifier_apply(modifier=mod.name)


report = []
for level, label in enumerate(['Original', 'Reduced', 'Map detail', 'Silhouette']):
    scene = bpy.data.scenes.new('Church LOD ' + str(level))
    bpy.context.window.scene = scene
    scene.unit_settings.system = 'METRIC'
    obj = original.copy()
    obj.data = original.data.copy()
    obj.name = 'Church_LOD' + str(level)
    scene.collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    simplify(obj, level)
    obj.data.calc_loop_triangles()
    report.append({'level': level, 'label': label, 'blenderVertices': len(obj.data.vertices), 'triangles': len(obj.data.loop_triangles), 'dimensionsMeters': list(obj.dimensions)})
    bpy.ops.export_scene.gltf(filepath=str(ROOT / ('church-lod' + str(level) + '.glb')), use_selection=True, use_active_scene=True, export_texcoords=False, export_cameras=False, export_lights=False)
(ROOT / 'blender-lod-stats.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))

# Identical framing makes the stored previews useful outside Storybook too.
from mathutils import Vector
for level in range(4):
    scene = bpy.data.scenes['Church LOD ' + str(level)]
    bpy.context.window.scene = scene
    bpy.ops.object.camera_add(location=(31, -39, 34))
    camera = bpy.context.object
    camera.rotation_euler = (Vector((0, 1, 5)) - camera.location).to_track_quat('-Z', 'Y').to_euler()
    camera.data.type = 'ORTHO'
    camera.data.ortho_scale = 35
    scene.camera = camera
    scene.render.engine = 'BLENDER_WORKBENCH'
    scene.render.resolution_x = 900
    scene.render.resolution_y = 900
    scene.render.resolution_percentage = 100
    scene.display.shading.light = 'STUDIO'
    scene.display.shading.color_type = 'MATERIAL'
    scene.display.shading.show_shadows = True
    scene.display.shading.show_cavity = True
    scene.display.shading.cavity_type = 'BOTH'
    scene.display.shading.background_type = 'WORLD'
    scene.world = bpy.data.worlds.new('Church backdrop')
    scene.world.color = (.075, .085, .10)
    scene.render.filepath = str(ROOT / ('lod' + str(level) + '-preview.png'))
    bpy.ops.render.render(write_still=True)
bpy.context.window.scene = bpy.data.scenes['Church LOD 0']
for screen in bpy.data.screens:
    for area in screen.areas:
        if area.type == 'VIEW_3D':
            area.spaces.active.shading.color_type = 'MATERIAL'
            area.spaces.active.region_3d.view_perspective = 'CAMERA'
bpy.ops.wm.save_as_mainfile(filepath=str(ROOT / 'church-lods.blend'))
