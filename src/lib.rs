extern crate git2;
extern crate typemap;
extern crate diecast;

#[macro_use]
extern crate log;
extern crate env_logger;

use std::sync::Arc;
use std::mem;

use diecast::{Item, Bind};

#[derive(Clone)]
pub struct Git {
    pub sha: git2::Oid,
    pub message: String,
}

impl typemap::Key for Git {
    type Value = Arc<Git>;
}

pub fn git(bind: &mut Bind) -> diecast::Result<()> {
    use std::collections::{HashMap, VecDeque};
    use git2::{
        Repository,
        Pathspec,
        Commit,
        DiffOptions,
        Error,
        Diff,
        Tree,
        Oid,
    };

    let repo = match Repository::open(".") {
        Ok(r) => r,
        Err(e) => {
            trace!("(git) {:?}: {}", bind, e);
            return Ok(());
        },
    };

    let mut diffopts = DiffOptions::new();
    diffopts.include_ignored(false);
    diffopts.recurse_ignored_dirs(false);
    diffopts.include_untracked(false);
    diffopts.recurse_untracked_dirs(false);
    diffopts.include_unmodified(false);
    diffopts.ignore_filemode(true);
    diffopts.ignore_submodules(true);
    diffopts.disable_pathspec_match(true);
    diffopts.skip_binary_check(true);
    diffopts.enable_fast_untracked_dirs(true);
    diffopts.include_unreadable(false);
    diffopts.force_text(true);

    let mut paths: VecDeque<(&mut Item, Pathspec)> = VecDeque::new();

    for item in bind.iter_mut() {
        let path = item.source().unwrap();

        diffopts.pathspec(path.to_str().unwrap());

        let pathspec = Pathspec::new(Some(path.to_str().unwrap()).into_iter()).unwrap();
        paths.push_back((item, pathspec));
    }

    let mut revwalk = repo.revwalk().unwrap();

    match revwalk.push_head() {
        Ok(_) => (),
        Err(e) => {
            trace!("(git): {}", e);
            return Ok(());
        },
    }

    let mut cache: HashMap<Oid, Arc<Git>> = HashMap::new();

    for id in revwalk {
        let id = try!(id);
        let commit = try!(repo.find_commit(id));
        let parents = commit.parents().len();

        // ignore merge commits
        if parents > 1 { continue }

        let is_root = parents == 0;

        fn match_with_parent<'a>(repo: &'a Repository, commit: &Commit, parent: &Commit,
                                 opts: &'a mut DiffOptions) -> Result<Diff<'a>, Error> {
            let a = try!(parent.tree());
            let b = try!(commit.tree());
            let diff = try!(repo.diff_tree_to_tree(Some(&a), Some(&b), Some(opts)));
            Ok(diff)
        }

        let remaining = mem::replace(&mut paths, VecDeque::new());

        let flags = git2::PATHSPEC_NO_MATCH_ERROR | git2::PATHSPEC_NO_GLOB;

        enum MatchKind<'a> {
            Tree(Tree<'a>),
            Diff(Diff<'a>),
        }

        let match_kind =
          if is_root {
              MatchKind::Tree(try!(commit.tree()))
          } else {
              MatchKind::Diff(try!(match_with_parent(&repo, &commit, &commit.parent(0).unwrap(), &mut diffopts)))
          };

        for (item, path) in remaining {
            let matched =
              match match_kind {
                  MatchKind::Tree(ref t) => path.match_tree(t, flags).is_ok(),
                  MatchKind::Diff(ref d) => path.match_diff(d, flags).is_ok(),
              };

            if !matched {
                paths.push_back((item, path));
                continue
            }

            let git =
                cache.entry(commit.id())
                .or_insert_with(|| {
                    let message = String::from_utf8_lossy(commit.message_bytes()).into_owned();
                    Arc::new(Git { sha: commit.id(), message: message })
                })
                .clone();

            item.extensions.insert::<Git>(git);
        }
    }

    Ok(())
}
