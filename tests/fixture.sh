#!/usr/bin/env bash
set -euo pipefail

root=$(realpath --canonicalize-missing "$1")
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_ADVICE=0
export GIT_AUTHOR_NAME='Ada Lovelace' GIT_AUTHOR_EMAIL=ada@example.com
export GIT_COMMITTER_NAME='Ada Lovelace' GIT_COMMITTER_EMAIL=ada@example.com
export GIT_AUTHOR_DATE='2026-01-02T03:04:05+02:00' GIT_COMMITTER_DATE='2026-01-02T03:04:05+02:00'

rm -rf "$root"
mkdir -p "$root"

build() {
	local name=$1 format=$2 filter=$3
	local bare=$root/$name.git work=$root/$name.work
	git init --quiet --bare --object-format="$format" --initial-branch=main "$bare"
	git -C "$bare" config uploadpack.allowFilter "$filter"
	git init --quiet --object-format="$format" --initial-branch=main "$work"
	printf 'one\n' >"$work/file"
	seq 1 400 >"$work/numbers"
	git -C "$work" add file numbers
	git -C "$work" commit --quiet --message first
	: >"$work/empty"
	{
		seq 1 200
		echo changed
		seq 201 400
	} >"$work/numbers-changed"
	git -C "$work" add empty numbers-changed
	git -C "$work" commit --quiet --message 'second

with a body'
	git -C "$work" tag light HEAD~1
	git -C "$work" tag --annotate --message annotated annotated
	git -C "$work" tag --annotate --message nested nested annotated
	git -C "$work" tag tree 'HEAD^{tree}'
	git -C "$work" tag --annotate --message 'tree tag' tree-tag 'HEAD^{tree}'
	git -C "$work" branch v2 HEAD~1
	git -C "$work" tag v2
	git -C "$work" branch heads/x HEAD~1
	git -C "$work" branch x
	git -C "$work" push --quiet --mirror "$bare"
}

build sha1 sha1 true
build sha256 sha256 true
build nofilter sha1 false
git init --quiet --bare --initial-branch=main "$root/empty.git"

latin1=$root/latin1.work
git init --quiet --initial-branch=main "$latin1"
printf 'x\n' >"$latin1/caf"$'\xe9'
git -C "$latin1" add .
printf 'Caf\xe9\n' >"$root/latin1.message"
GIT_AUTHOR_NAME=$'\xc9mile' git -C "$latin1" -c i18n.commitEncoding=ISO-8859-1 commit --quiet --file "$root/latin1.message"
git init --quiet --bare --initial-branch=main "$root/latin1.git"
git -C "$latin1" push --quiet --mirror "$root/latin1.git"

deep=$root/deep.work
git init --quiet --initial-branch=main "$deep"
git -C "$deep" commit --quiet --allow-empty --message deep
git -C "$deep" tag --annotate --message t0 t0
for n in $(seq 1 16); do
	git -C "$deep" tag --annotate --message "t$n" "t$n" "t$((n - 1))"
done
git init --quiet --bare --initial-branch=main "$root/deep.git"
git -C "$deep" push --quiet --mirror "$root/deep.git"
