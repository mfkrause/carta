#let plain = read("data-text.txt")
#let rows = csv("data-csv.csv")
#let j = json("data-json.json")
#let t = toml("data-toml.toml")
#let y = yaml("data-yaml.yaml")
#let x = xml("data-xml.xml")

Read: #plain.len() characters.

Csv: #rows.len() rows, cell #rows.at(1).at(1).

Json: #j.k, #j.s, #j.a.at(1).

Toml: #t.title, #t.n, #t.owner.name.

Yaml: #y.k, #y.list.at(1).

Xml: #x.at(0).tag, #x.at(0).attrs.a, #x.at(0).children.at(0).
