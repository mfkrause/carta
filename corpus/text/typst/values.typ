#let double = (x) => x * 2
#let label = name => [item #name]
Doubled: #double(4), #label("one").

#let sizes = (2, 3, 4)
Items: #sizes.at(1), #sizes.len(), #sizes.first(), #sizes.last(), #sizes.sum().
Ordered: #sizes.enumerate(), #sizes.rev(), #sizes.slice(1, 3), #sizes.join(" and ").

#let entry = (title: "Report", pages: 12)
Fields: #entry.title, #entry.at("pages"), #entry.keys(), #entry.values().

Repeated: #("ab" * 3), #([x] * 2).

Converted: #str(42), #int("7"), #float("1.5"), #repr((1, 2)), #type(1.5), #eval("1 + 1").

Joined: #([a] + [b]), #([x] + "y"), #((1, 2) + (3,)).
