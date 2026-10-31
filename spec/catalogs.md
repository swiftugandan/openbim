# Product catalogues

Frozen contract. A catalogue is a manufacturer's or brand's product list: doors, windows, furniture, wall finishes such
as paint, floor finishes, ceiling finishes, roof coverings and materials. Catalogues are data, so a new manufacturer
needs no rebuild.

## Catalogue files

A catalogue lives in `<dir>/catalogs/<catalog>/catalog.json` (`<catalog>` matches `[a-z][a-z0-9-]*` and equals the
`catalog` field):

```json
{"catalog": "acme", "version": "2026.1", "manufacturer": "Acme Building Products Ltd",
 "products": [
   {"sku": "FD30-926", "category": "door", "name": "Oak fire door 926 x 2040",
    "props": {"width": 0.926, "height": 2.04, "fire_rating": "FD30", "u_value": 1.8}},
   {"sku": "CW-1210", "category": "window", "name": "Casement window 1210 x 1050",
    "props": {"width": 1.21, "height": 1.05, "u_value": 1.2, "glazing": "double"}},
   {"sku": "SOFA-3", "category": "furniture", "name": "Three-seat sofa",
    "props": {"size": [2.1, 0.9, 0.85], "colour": "slate"}},
   {"sku": "EM-WHT", "category": "wall_finish", "name": "Matt emulsion, white", "props": {"colour": "white", "coats": 2}},
   {"sku": "OAK-14", "category": "floor_finish", "name": "Engineered oak 14 mm", "props": {"thickness": 0.014}},
   {"sku": "SL-NAT", "category": "roof_covering", "name": "Natural slate", "props": {"mass_per_area": 25}}
 ]}
```

- Required at the top level: `catalog`, `version` and `manufacturer`. Required on every product: `sku` (unique in the catalogue, `[A-Za-z0-9._-]+`), `category`, `name`. `manufacturer`
  defaults to the catalogue's. `props` values are numbers, strings, booleans or lists of numbers. A
  `colour` prop, where present, is a CSS colour: `#rrggbb` or a CSS named colour.
- Categories with defined behaviour: `door` and `window` (props `width`, `height` required), `furniture` (`size`
  required: [width, depth, height] in metres), `wall_finish`, `floor_finish`, `ceiling_finish`, `roof_covering`,
  `material`. Any other category is allowed and carries no special behaviour.
- Malformed JSON, a missing required field, a duplicate `sku` or a directory name that differs from `catalog` makes a
  `bim catalog` command exit 2 naming the file. Engines never read catalogue files; they read `Product` entities.

## Using a product

`bim catalog use <dir> <catalog>:<sku>` copies the product into the document as a `Product` entity whose id is
`<catalog>:<sku>`, and prints that id as its last line on stdout. An unknown `<catalog>` or `<sku>` exits 2 with
nothing changed. Running it again replaces the entity's props with the catalogue's current
values in one atomic apply, so every element that references the product follows the new version.
`bim catalog list <dir> --json` prints
`{"catalogs":[{"catalog","version","manufacturer","products":[{"sku","category","name"}]}]}`, catalogues sorted by
name and products by `sku`.

A `Product` entity (spec/schema.md) holds `catalog`, `catalog_version`, `sku`, `category`, `name`, `manufacturer` and
each of the product's `props` under its own key.

## Elements that reference products

- `Opening.product`: a door or window product. `Furniture.product`: a furniture product.
- `Wall.finish_left`, `Wall.finish_right`: wall finishes on each face (left looking from (x1,y1) to (x2,y2)).
- `Slab.finish_top`: a floor finish. `Slab.finish_bottom`: a ceiling finish. `Roof.covering`: a roof covering.

**Effective props.** Where an element has a `product`, each prop the element itself lacks takes the value of the
product's prop with the same key; the element's own value always wins. Geometry, engines, code rule packs, IFC export and
the Studio all use effective props. Finishes and coverings have no thickness in geometry.

## Take-off

`bim solve <dir> --trade arch --json` adds
`"products":[{"id","category","count","area"}]` sorted by id, one entry per Product referenced by any element:

- `count` = number of references to it (a Wall with the same finish on both faces counts twice).
- `area` = for a wall finish, the wall's `side_area` per referencing face; for a floor or ceiling finish, the slab's
  net plan area (`slabs[].area`); for a roof covering, the roof's sloped area; 0 for doors, windows and furniture.

and `"roofs":[{"id","area","volume"}]` sorted by id (`area` sloped top area, spec/schema.md Roof rule).
