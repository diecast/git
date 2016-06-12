extern crate diecast;
extern crate env_logger;
extern crate git2;
extern crate typemap;

#[macro_use]
extern crate log;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::{mem, str};

use diecast::{Item, Bind, Handle};

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

pub struct GitCommit  {
    revision: String,
}

impl GitCommit {
    /// Determine the last commit that touched a given file starting from the
    /// specified revision.
    ///
    /// For example, if `"origin/master"` is used, only commits that are known
    /// to be available at the `origin` remote's `master` branch will be
    /// considered. This is useful so that only commits that have been pushed
    /// are considered.
    pub fn from_revision(revision: &str) -> GitCommit {
        GitCommit { revision: String::from(revision) }
    }

    /// Convenience method that uses `"HEAD"` as the revision for the above
    /// method.
    ///
    /// Note that this means that it is possible for it to use commits which
    /// haven't yet been pushed, which would result in broken links for example
    /// if you're linking to the commit on GitHub.
    pub fn from_head() -> GitCommit {
        GitCommit { revision: String::from("HEAD") }
    }
}

#[derive(Clone)]
pub struct LastCommit {
    pub sha: String,
    pub summary: String,
    pub time: git2::Time,
}

impl typemap::Key for LastCommit {
    type Value = Arc<LastCommit>;
}

impl Handle<Bind> for GitCommit {
    fn handle(&self, bind: &mut Bind) -> diecast::Result<()> {
        // since this uses discover, the git repo is looked up the
        // fs hierarchy until one is found. this means that even with github-pages'
        // behavior of making cwd .build/, it'll find the git repo at the root
        let repo = match Repository::discover(".") {
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

            let p = Some(path.to_str().unwrap());
            let pathspec = Pathspec::new(p.into_iter()).unwrap();
            paths.push_back((item, pathspec));
        }

        let mut revwalk = repo.revwalk().unwrap();

        let commit = try!(repo.revparse_single(&self.revision)
                          .and_then(|r| r.peel(git2::ObjectType::Commit)));

        match revwalk.push(commit.id()) {
            Ok(_) => (),
            Err(e) => {
                trace!("(git): {}", e);
                return Ok(());
            },
        }

        let mut cache: HashMap<Oid, Arc<LastCommit>> = HashMap::new();

        for id in revwalk {
            let id = try!(id);
            let mut commit = try!(repo.find_commit(id));
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
                        let summary = String::from(commit.summary().unwrap());
                        let sha = commit.id().to_string();

                        Arc::new(LastCommit {
                            sha: sha,
                            summary: summary,
                            time: commit.time(),
                        })
                    })
                    .clone();

                item.extensions.insert::<LastCommit>(git);
            }
        }

        Ok(())
    }

}
