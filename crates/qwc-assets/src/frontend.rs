//! The front end's resources on the disc: the `_ResourceData` records of the layer scripts
//! (`qwc_formats::vso::Resource`: an id, a group and a name) to the files they load.
//!
//! The original resolves a record through its group's loaded package (`0x4ea5b0`): each package of the exe's
//! load-set table is registered under a group (`qwc_formats::flow::loadset_table`; `FE_Loadsrcn_FE` is the
//! package `Frontend\Loadscrn\fe`). Given that table ([`Resources::build_in`]), a record's file is its group's
//! package folder plus its name; a group with several packages (the WB licence per language, the analog stick
//! pictures) keeps each as an alternative, and [`Resources::path_in`] takes the one loaded. **[I]** without the
//! table (it is only in a decrypted exe), or for a name no package holds, a group `FE_A_B` is the folder
//! `frontend/A/B`, failing that a file of the name found once on the disc, or else the one of them whose folders the
//! group's parts begin (`FE_Common_Logo_English` is `frontend/Common/Logos/ENGLISH`). Types: 0 a texture (`name.fsh`), 1 a font (`frontend/Common/Fonts/SET/name.ffn`
//! for a font set `SETFonts`, Latin for `FE_Common_Fonts`), 4 a model (`name.fpm`), 5 a wipe (`name`, already
//! `.wpe`), 6 a puppet's model (`name.fpm`) or clips (`name.fas`, its skeleton `name.skl` beside it).
//!
//! A record with choices (`qwc_formats::vso::Choices`: the title's logo per language, the fonts per font set)
//! takes the entry a function of its script answers, as it registers (`0x4ea420`); [`Resources::build_with`] is
//! given a way to run those functions, and [`Resources::refresh_choices`] runs them again (the original runs them as
//! each screen loads its layers: the House Cup's players are the house the DataStore holds then).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::Vfs;

/// The index over every script's records. Ids are a script's own: 71 of the 627 name other files in other scripts
/// (`mainmenu_specmask` and `TB_template_specmask` share one), and the original looks an id up in the tables of the
/// layer's screen (`0x4ea5b0`). So each script's records are kept by its hash ([`Resources::path_of`]), and by id
/// alone the first script's that resolves.
#[derive(Debug, Clone, Default)]
pub struct Resources {
    /// The file each record names, by id (the first script's that resolves).
    pub paths: HashMap<u32, String>,
    /// Each record's type, by id.
    pub kinds: HashMap<u32, u32>,
    /// The ids from each record to the end of its script's list (an animated sprite's frame `k` is the `k`-th).
    following: HashMap<u32, Vec<u32>>,
    /// Records whose group has several packages holding the file: (package folder, file), by id. `paths` holds
    /// the first.
    alternatives: HashMap<u32, Vec<(String, String)>>,
    /// Each script's records, by (script hash, id): the file found and the alternatives, and the ids following.
    local: HashMap<(u32, u32), Found>,
    local_following: HashMap<(u32, u32), Vec<u32>>,
    /// Each group's package folders and every texture, model and clip file by name: what [`Resources::texture`]
    /// resolves a group's name with.
    packages: HashMap<String, Vec<String>>,
    by_name: HashMap<String, Vec<String>>,
    /// The records with choices, by (script hash, id): the script's index, the function, and each entry resolved.
    choices: HashMap<(u32, u32), (usize, u32, Vec<Found>)>,
    /// The entry each of those last answered (none: the record's own fields), shared by the index's clones.
    chosen: Rc<RefCell<HashMap<(u32, u32), usize>>>,
}

/// One record resolved: its file (the first package's) and, with several packages holding it, each
/// (folder, file).
#[derive(Debug, Clone, Default)]
struct Found {
    path: Option<String>,
    alternatives: Vec<(String, String)>,
    /// The record's name, lower case (a file another script's record of the id found stands in only with it).
    name: String,
}

impl Resources {
    /// Index the records of `scripts` (the `.vso` files' bytes); scripts that do not parse are skipped. Records
    /// with choices keep their own fields.
    pub fn build<'a>(vfs: &Vfs, scripts: impl IntoIterator<Item = &'a [u8]>) -> Resources {
        Resources::build_with(vfs, scripts, |_, _| None)
    }

    /// As [`Resources::build`], with `run(script, function)` running a function of the `script`-th script (by
    /// its hash) for the records with choices.
    pub fn build_with<'a>(
        vfs: &Vfs,
        scripts: impl IntoIterator<Item = &'a [u8]>,
        run: impl FnMut(usize, u32) -> Option<u32>,
    ) -> Resources {
        Resources::build_in(vfs, scripts, run, &HashMap::new())
    }

    /// As [`Resources::build_with`], with `packages`: each group's package folders (from the load-set table,
    /// `/` separated).
    pub fn build_in<'a>(
        vfs: &Vfs,
        scripts: impl IntoIterator<Item = &'a [u8]>,
        mut run: impl FnMut(usize, u32) -> Option<u32>,
        packages: &HashMap<String, Vec<String>>,
    ) -> Resources {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for ext in ["fsh", "wpe", "fpm", "fas"] {
            for f in vfs.with_extension(ext) {
                let file = f.rsplit('/').next().unwrap_or(f).to_ascii_lowercase();
                by_name.entry(file).or_default().push(f.to_string());
            }
        }
        let mut r = Resources::default();
        for (script, b) in scripts.into_iter().enumerate() {
            let Ok(v) = qwc_formats::vso::Vso::parse(b) else { continue };
            let Ok(res) = v.resources() else { continue };
            for (k, rec) in res.iter().enumerate() {
                // **[I]** without a value (or past the table) the record keeps its own fields
                let own = (rec.group.as_str(), rec.name.as_str(), rec.kind);
                let (group, name, kind) = match &rec.choices {
                    None => own,
                    Some(c) => {
                        let all = c.entries.iter().map(|(g, n, t)| Found {
                            name: n.to_ascii_lowercase(),
                            ..resolve(vfs, packages, &by_name, g, n, *t)
                        });
                        r.choices.insert((v.hash, rec.id), (script, c.function, all.collect()));
                        let at = run(script, c.function).filter(|&v| (v as usize) < c.entries.len());
                        match at.map(|v| (v as usize, &c.entries[v as usize])) {
                            Some((k, (g, n, t))) => {
                                r.chosen.borrow_mut().insert((v.hash, rec.id), k);
                                (g.as_str(), n.as_str(), *t)
                            }
                            None => own,
                        }
                    }
                };
                let following: Vec<u32> = res[k..].iter().map(|x| x.id).collect();
                r.local_following.entry((v.hash, rec.id)).or_insert_with(|| following.clone());
                r.following.entry(rec.id).or_insert(following);
                r.kinds.entry(rec.id).or_insert(kind);
                let found =
                    Found { name: name.to_ascii_lowercase(), ..resolve(vfs, packages, &by_name, group, name, kind) };
                if let (std::collections::hash_map::Entry::Vacant(e), Some(p)) = (r.paths.entry(rec.id), &found.path) {
                    e.insert(p.clone());
                    if found.alternatives.len() > 1 {
                        r.alternatives.insert(rec.id, found.alternatives.clone());
                    }
                }
                r.local.entry((v.hash, rec.id)).or_insert(found);
            }
        }
        r.packages = packages.clone();
        r.by_name = by_name;
        r
    }

    /// Run the choice functions again (`run(script, function)`, as at the build) and take the entries they answer
    /// now; a function that answers nothing keeps its entry. Answers whether any changed.
    pub fn refresh_choices(&self, mut run: impl FnMut(usize, u32) -> Option<u32>) -> bool {
        let mut changed = false;
        let mut chosen = self.chosen.borrow_mut();
        for (key, (script, function, all)) in &self.choices {
            let Some(k) = run(*script, *function).map(|v| v as usize).filter(|&k| k < all.len()) else { continue };
            if chosen.insert(*key, k) != Some(k) {
                changed = true;
            }
        }
        changed
    }

    /// Script `script`'s record `id` as its choice function last answered, for a record with choices.
    fn chosen(&self, script: u32, id: u32) -> Option<&Found> {
        let k = *self.chosen.borrow().get(&(script, id))?;
        self.choices.get(&(script, id))?.2.get(k)
    }

    /// The texture `name` of resource group `group` (`FE_TeamTent_VCPC00`, `chasers`), found as a record of
    /// theirs would be: what a native that loads a texture by its group and name finds
    /// (`General_SetQuidditchCardTexture`).
    pub fn texture(&self, vfs: &Vfs, group: &str, name: &str) -> Option<String> {
        resolve(vfs, &self.packages, &self.by_name, group, name, 0).path
    }

    /// Record `id`'s `frame` as script `script` (its `.vso` hash) names it: its own record, else the first
    /// script's. The original looks in the tables of the layer's screen.
    pub fn path_of(&self, script: u32, id: u32, frame: u8) -> Option<&str> {
        self.path_in_script(script, id, frame, &|_| false)
    }

    /// As [`Resources::path_of`], taking a record with several packages from the first `loaded` holds.
    pub fn path_in_script(&self, script: u32, id: u32, frame: u8, loaded: &dyn Fn(&str) -> bool) -> Option<&str> {
        let Some(f) = self.local_following.get(&(script, id)) else { return self.path_in(id, frame, loaded) };
        let id = *f.get(frame as usize)?;
        let Some(found) = self.chosen(script, id).or_else(|| self.local.get(&(script, id))) else {
            return self.path_in(id, 0, loaded);
        };
        if found.path.is_none() {
            // **[I]** a record no rule places takes the file another script's record of the id found, when that is
            // a file of its name (its group's folder is elsewhere); never another name's
            let other = self.path_in(id, 0, loaded)?;
            let file = other.rsplit('/').next().unwrap_or(other).to_ascii_lowercase();
            return file
                .strip_prefix(&found.name)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
                .then_some(other);
        }
        if let Some((_, p)) = found.alternatives.iter().find(|(folder, _)| loaded(folder)) {
            return Some(p);
        }
        found.path.as_deref()
    }

    /// The record `frame` places after `id` in its script's list (`id` itself for frame 0).
    pub fn frame(&self, id: u32, frame: u8) -> Option<u32> {
        self.following.get(&id)?.get(frame as usize).copied()
    }

    /// The file of record `id`'s `frame`.
    pub fn path(&self, id: u32, frame: u8) -> Option<&str> {
        self.paths.get(&self.frame(id, frame)?).map(String::as_str)
    }

    /// As [`Resources::path`], taking a record with several packages from the first `loaded` holds (package
    /// folders, as given to [`Resources::build_in`]).
    pub fn path_in(&self, id: u32, frame: u8, loaded: &dyn Fn(&str) -> bool) -> Option<&str> {
        let id = self.frame(id, frame)?;
        if let Some(alt) = self.alternatives.get(&id) {
            if let Some((_, p)) = alt.iter().find(|(folder, _)| loaded(folder)) {
                return Some(p);
            }
        }
        self.paths.get(&id).map(String::as_str)
    }
}

/// A record's file: in its group's packages (each that holds it), else **[I]** the folder its group names, a file of
/// its name found once, or the one of several whose folders its group fits.
fn resolve(
    vfs: &Vfs,
    packages: &HashMap<String, Vec<String>>,
    by_name: &HashMap<String, Vec<String>>,
    group: &str,
    name: &str,
    kind: u32,
) -> Found {
    let files = match kind {
        0 => vec![format!("{name}.fsh")],
        1 => {
            // a font set `XFonts` is the folder `frontend/Common/Fonts/X`; `FE_Common_Fonts` is Latin
            let set = group.strip_suffix("Fonts").filter(|x| !x.is_empty() && !x.contains('_'));
            let p = format!("frontend/Common/Fonts/{}/{name}.ffn", set.unwrap_or("Latin"));
            return Found { path: vfs.exists(&p).then_some(p), ..Default::default() };
        }
        4 => vec![format!("{name}.fpm")],
        5 => vec![name.to_string()],
        // a puppet's model, or its clips (`name.fas`, with `name.skl` beside it)
        6 => vec![format!("{name}.fpm"), format!("{name}.fas")],
        _ => vec![],
    };
    for file in files {
        let held: Vec<(String, String)> = packages
            .get(group)
            .into_iter()
            .flatten()
            .map(|folder| (folder.clone(), format!("{folder}/{file}")))
            .filter(|(_, p)| vfs.exists(p))
            .collect();
        if let Some((_, first)) = held.first() {
            let path = Some(first.clone());
            let alternatives = if held.len() > 1 { held } else { vec![] };
            return Found { path, alternatives, ..Default::default() };
        }
        let dir = group.strip_prefix("FE_").map(|g| format!("frontend/{}/", g.replace('_', "/")));
        let path = dir.map(|d| format!("{d}{file}")).filter(|p| vfs.exists(p)).or_else(|| {
            match by_name.get(&file.to_ascii_lowercase()).map(Vec::as_slice) {
                Some([p]) => Some(p.clone()),
                Some(many) => {
                    let fits: Vec<&String> = many.iter().filter(|p| group_fits(group, p)).collect();
                    (fits.len() == 1).then(|| fits[0].clone())
                }
                None => None,
            }
        });
        if path.is_some() {
            return Found { path, ..Default::default() };
        }
    }
    Found::default()
}

/// **[I]** Whether a group names a file's folder part by part, each a case-insensitive prefix of the folder's
/// (`FE_Common_Logo_English` fits `frontend/Common/Logos/ENGLISH/qwc_logo.fsh`).
fn group_fits(group: &str, path: &str) -> bool {
    let Some(rest) = group.strip_prefix("FE_") else { return false };
    let parts: Vec<String> = rest.split('_').map(str::to_ascii_lowercase).collect();
    let dirs: Vec<String> = path.split('/').map(str::to_ascii_lowercase).collect();
    let Some((_, dirs)) = dirs.split_last() else { return false };
    dirs.len() == parts.len() + 1
        && dirs[0] == "frontend"
        && parts.iter().zip(&dirs[1..]).all(|(p, d)| d.starts_with(p.as_str()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_fit_their_folders() {
        use super::group_fits;
        assert!(group_fits("FE_Common_Logo_English", "frontend/Common/Logos/ENGLISH/qwc_logo.fsh"));
        assert!(!group_fits("FE_Common_Logo_English", "frontend/Common/Logos/US/qwc_logo.fsh"));
        assert!(!group_fits("FE_Common_Logo", "frontend/Common/Logos/US/qwc_logo.fsh"));
        assert!(!group_fits("Common_Logo_US", "frontend/Common/Logos/US/qwc_logo.fsh"));
    }
}
