#let phrase = "one two three"
Found: #phrase.find("two"), at #phrase.position("two").
Bounds: #phrase.starts-with("one"), #phrase.ends-with("three"), #phrase.contains("tw").
Parts: #phrase.split(" ").at(1), #phrase.slice(4, 7), #phrase.replace("two", "2").
Letters: #"héllo".codepoints().len(), #"abc".clusters().at(0).
