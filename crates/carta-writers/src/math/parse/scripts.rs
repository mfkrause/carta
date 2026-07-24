//! Script-chain binding: attaching sub/superscripts and primes to the most recent atom of a run.

use super::{Atom, Body, MAX_DEPTH, ScriptKind, Sibling};

/// State for binding a chain of scripts to the most recent atom of a run.
///
/// `path` descends from the run's last atom (the base) to the *active* atom, where the next nesting
/// script lands: the first step optionally selects a sibling group of the base, and each remaining
/// step takes the last element of the active atom's subscript or superscript group. `last` is the
/// kind of the script applied most recently, used to decide whether an occupied-slot script nests
/// (consecutive same kind) or siblings. `sealed` is set after a script lands across a group boundary
/// (`{x^2}…`): every later script then siblings in source order instead of filling a primary slot.
#[derive(Default)]
pub(super) struct ScriptChain {
    /// `Some(i)` when the active atom lives inside the base's `i`th sibling group; the active is the
    /// last atom of that group descended through `steps`.
    pub(super) sibling: Option<usize>,
    pub(super) steps: Vec<ScriptKind>,
    pub(super) last: Option<ScriptKind>,
    pub(super) sealed: bool,
}

/// Bind a script of `kind` to `base` (the run's last atom), following `chain` to the active atom.
///
/// Scripts written in sequence build a tree. The *active* atom is the last atom of the script
/// content applied most recently; `chain` records the path to it. A script that repeats the previous
/// kind nests one level deeper into the active atom (`a^b^c` → `a^{b^c}`); a script of the other kind
/// applies to the active atom's parent, so a fresh base collects both its scripts (`a^b_c`). Across a
/// group boundary the base is sealed: a non-nesting script siblings the base in source order
/// (`{x^2}_3`) instead of filling a primary slot.
pub(super) fn attach_script(
    base: &mut Atom,
    chain: &mut ScriptChain,
    kind: ScriptKind,
    script: Vec<Atom>,
) -> Option<()> {
    // The chain's first script fills the base's free matching slot; sealed or occupied falls through.
    if chain.last.is_none() && !chain.sealed {
        let free = match kind {
            ScriptKind::Sub => base.sub.is_none(),
            ScriptKind::Sup => base.sup.is_none(),
        };
        if free {
            let descent = empty_base_descent(&script);
            match kind {
                ScriptKind::Sub => base.sub = Some(script),
                ScriptKind::Sup => base.sup = Some(script),
            }
            chain.steps.push(kind);
            chain.steps.extend(descent);
            chain.last = chain.steps.last().copied();
            return Some(());
        }
    }
    // Reject over-deep chains (`a^a^a^…`) before rendering could overflow the stack.
    if chain.steps.len() >= MAX_DEPTH {
        return None;
    }
    // A repeated kind nests one level deeper into the active atom; nothing nests into a prime, so a
    // script on a prime active siblings the base instead.
    if chain.last == Some(kind) && !active_atom(base, chain).is_some_and(|a| a.is_prime()) {
        let descent = empty_base_descent(&script);
        let active = active_atom(base, chain)?;
        match kind {
            ScriptKind::Sub if active.sub.is_none() => active.sub = Some(script),
            ScriptKind::Sup if active.sup.is_none() => active.sup = Some(script),
            _ => return None,
        }
        chain.steps.push(kind);
        chain.steps.extend(descent);
        chain.last = chain.steps.last().copied();
        return Some(());
    }
    // The other kind reaches one step up so the parent pairs both scripts; only after at least one
    // descent, never at the root or when sealed (those sibling instead).
    if !chain.sealed && !chain.steps.is_empty() {
        chain.steps.pop();
        let parent = active_atom(base, chain)?;
        let free = match kind {
            ScriptKind::Sub => parent.sub.is_none(),
            ScriptKind::Sup => parent.sup.is_none(),
        };
        if free {
            match kind {
                ScriptKind::Sub => parent.sub = Some(script),
                ScriptKind::Sup => parent.sup = Some(script),
            }
            chain.steps.push(kind);
            chain.last = Some(kind);
            return Some(());
        }
    }
    // Otherwise the script siblings the base in source order and becomes the active group.
    base.siblings.push(Sibling {
        kind,
        atoms: script,
        sealed: chain.sealed,
    });
    chain.sibling = Some(base.siblings.len() - 1);
    chain.steps.clear();
    chain.last = Some(kind);
    Some(())
}

/// Append a prime mark to an atom outside a script chain (matrix cells, `\left…\right` runs): merge
/// into a trailing prime superscript when one is already present, else add a fresh prime sibling.
pub(super) fn push_prime(atom: &mut Atom) {
    if let Some(sibling) = atom.siblings.last_mut()
        && sibling.kind == ScriptKind::Sup
        && let Some(last) = sibling.atoms.last_mut()
        && let Body::Prime(count) = &mut last.body
    {
        *count = count.saturating_add(1);
        return;
    }
    if atom.sup.is_none() && atom.siblings.is_empty() {
        atom.sup = Some(vec![Atom::new(Body::Prime(1))]);
    } else {
        atom.siblings.push(Sibling {
            kind: ScriptKind::Sup,
            atoms: vec![Atom::new(Body::Prime(1))],
            sealed: false,
        });
    }
}

/// Whether a prime mark detaches from `base` to surface as a bare prime glyph rather than nesting as
/// a superscript. A prime is a superscript; it nests when the chain offers a free superscript slot,
/// either on the active atom (a repeated-superscript nest) or on its parent (the matching-pair reach
/// up one level). When neither offers a slot the superscript would have to start a fresh sibling
/// group, with nowhere to nest; there TeX detaches the prime, so it surfaces as a bare glyph that
/// starts a new base. The shapes this covers include `a_b'` and the mirror `a_b^c'`, as well as the
/// deeper `a^c'_d'`, where the active atom already carries a primary prime.
///
/// A sealed base (`{…}'`) or a chain already pointing at a sibling group keeps its prime nested, so
/// only an unsealed primary chain can detach.
pub(super) fn prime_detaches(base: &Atom, chain: &ScriptChain) -> bool {
    if chain.sealed || chain.sibling.is_some() {
        return false;
    }
    // At the chain root a prime fills the base's own free superscript slot, so it never detaches.
    if chain.last.is_none() {
        return false;
    }
    let Some(active) = active_atom_ref(base, chain) else {
        return false;
    };
    // A prime on an active prime merges into its count (`a''`, `a_b''`), never detaches.
    if active.is_prime() {
        return false;
    }
    // One step from the base: detach once both primary slots are filled (`a'_b'`, `a_b^c'`).
    if chain.steps.len() == 1 && base.sub.is_some() && base.sup.is_some() {
        return true;
    }
    // A repeated superscript nests onto the active atom when its slot is free.
    if chain.last == Some(ScriptKind::Sup) && active.sup.is_none() {
        return false;
    }
    // Otherwise reach up: nest when the parent's sup slot is free, detach when taken (`a^c'_d'`).
    parent_atom_ref(base, chain).is_none_or(|parent| parent.sup.is_some())
}

/// The atom one chain step above the active atom: the target of a matching-pair script that reaches
/// up a level. `None` when the active atom is the chain root (the base itself), which has no parent.
fn parent_atom_ref<'a>(base: &'a Atom, chain: &ScriptChain) -> Option<&'a Atom> {
    let parent_steps = chain.steps.split_last()?.1;
    descend_ref(base, chain.sibling, parent_steps)
}

/// Attach one prime mark to `base`, flowing it through `chain` as a superscript so its place among
/// other scripts is preserved. Consecutive primes merge into the count of a single [`Body::Prime`]
/// atom rather than nesting (`a''` is one double-prime, not a prime on a prime).
pub(super) fn attach_prime(base: &mut Atom, chain: &mut ScriptChain) -> Option<()> {
    if chain.last == Some(ScriptKind::Sup)
        && let Some(active) = active_atom(base, chain)
        && let Body::Prime(count) = &mut active.body
    {
        *count = count.saturating_add(1);
        return Some(());
    }
    attach_script(
        base,
        chain,
        ScriptKind::Sup,
        vec![Atom::new(Body::Prime(1))],
    )
}

/// The chain of script steps that descends from a freshly-attached script's root to the deepest real
/// atom through any synthesized empty nuclei. A script written as a bare operator chain (`a^_b`,
/// `a__b`) attaches as one or more nested [`Body::Empty`] bases; a following flat script must bind
/// onto the deepest of those, so the chain descends through each empty's sole filled slot. Descent
/// stops at the first non-empty nucleus (the real script content), which becomes the new active base.
fn empty_base_descent(script: &[Atom]) -> Vec<ScriptKind> {
    let mut steps = Vec::new();
    let mut current = script;
    while let [atom] = current {
        if !matches!(atom.body, Body::Empty) {
            break;
        }
        if let Some(inner) = atom.sub.as_deref() {
            steps.push(ScriptKind::Sub);
            current = inner;
        } else if let Some(inner) = atom.sup.as_deref() {
            steps.push(ScriptKind::Sup);
            current = inner;
        } else {
            break;
        }
    }
    steps
}

/// The read-only twin of [`active_atom`]: resolve the atom `chain` currently points at within `base`,
/// descending through the chosen sibling group and each script step without taking a mutable borrow.
fn active_atom_ref<'a>(base: &'a Atom, chain: &ScriptChain) -> Option<&'a Atom> {
    descend_ref(base, chain.sibling, &chain.steps)
}

/// Resolve an atom by descending from `base` into the optional sibling group, then following each
/// script `step` into the last atom of the matching slot. The read-only core shared by the active-
/// and parent-atom resolvers.
fn descend_ref<'a>(
    base: &'a Atom,
    sibling: Option<usize>,
    steps: &[ScriptKind],
) -> Option<&'a Atom> {
    let mut atom = match sibling {
        Some(i) => base.siblings.get(i)?.atoms.last()?,
        None => base,
    };
    for step in steps {
        let group = match step {
            ScriptKind::Sub => atom.sub.as_deref()?,
            ScriptKind::Sup => atom.sup.as_deref()?,
        };
        atom = group.last()?;
    }
    Some(atom)
}

/// Resolve the active atom that `chain` currently points at within `base`: descend into the chosen
/// sibling group (or the base itself), then follow each step into the last atom of that script group.
fn active_atom<'a>(base: &'a mut Atom, chain: &ScriptChain) -> Option<&'a mut Atom> {
    let mut atom = match chain.sibling {
        Some(i) => base.siblings.get_mut(i)?.atoms.last_mut()?,
        None => base,
    };
    for step in &chain.steps {
        let group = match step {
            ScriptKind::Sub => atom.sub.as_mut()?,
            ScriptKind::Sup => atom.sup.as_mut()?,
        };
        atom = group.last_mut()?;
    }
    Some(atom)
}
