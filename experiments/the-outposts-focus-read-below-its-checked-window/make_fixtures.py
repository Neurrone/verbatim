"""Writes the study's four generated mockapp fixtures into the current folder:
flat lists of 150, 250 and 400 items, and a list of 120 items with four
cells each, like a file list's details view."""
import json


def item(i, cells=0):
    node = {"id": "item%d" % i, "role": "list_item", "name": "Item %03d" % i,
            "states": ["focusable", "selectable"]}
    if cells:
        node["children"] = [{"id": "cell%d_%d" % (i, k), "role": "static_text",
                             "name": "Cell %d %d" % (i, k)} for k in range(cells)]
    return node


for n in (150, 250, 400):
    tree = {"id": "root", "role": "window", "name": "Study Wide %d" % n,
            "children": [{"id": "many", "role": "list", "name": "Many",
                          "children": [item(i) for i in range(n)]}]}
    json.dump(tree, open("study_wide%d.json" % n, "w"), indent=1)
tree = {"id": "root", "role": "window", "name": "Study Grid",
        "children": [{"id": "many", "role": "list", "name": "Files",
                      "children": [item(i, 4) for i in range(120)]}]}
json.dump(tree, open("study_grid120.json", "w"), indent=1)
