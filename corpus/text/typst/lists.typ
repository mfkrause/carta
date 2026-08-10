- first bullet
- second bullet
  - nested bullet
  - another nested
- third bullet

+ first numbered
+ second numbered
  + nested numbered

3. explicit start
4. next explicit

/ Term: the definition body.
/ Other term: another definition.

#set enum(numbering: "a)")

+ lettered first
+ lettered second

#set enum(numbering: "1.", start: 4)

+ starts at four
+ and five

#enum(numbering: "(I)", [roman one], [roman two])
