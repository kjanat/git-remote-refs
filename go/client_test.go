package gitremoterefs

import (
	"bytes"
	"compress/zlib"
	"context"
	"crypto/sha1"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/http/cgi"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/kjanat/git-remote-refs/go/internal/kjanat/git-remote-refs/git"
	"github.com/kjanat/wacogo"
)

var (
	root    string
	backend http.Handler
	v2      *httptest.Server
	v0      *httptest.Server
	shared  *Client
	gitEnv  = []string{"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL=/dev/null", "GIT_TERMINAL_PROMPT=0"}
	resultT = "application/x-git-upload-pack-result"
)

func TestMain(m *testing.M) {
	os.Exit(run(m))
}

func run(m *testing.M) (code int) {
	dir, err := os.MkdirTemp("", "git-remote-refs-go-")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer func() {
		if err := os.RemoveAll(dir); err != nil {
			fmt.Fprintln(os.Stderr, err)
			code = 1
		}
	}()
	root = filepath.Join(dir, "fixtures")
	script := exec.Command("bash", "../tests/fixture.sh", root)
	script.Stdout, script.Stderr = os.Stderr, os.Stderr
	if err := script.Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	path, err := exec.LookPath("git")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	backend = &cgi.Handler{
		Path: path,
		Args: []string{"http-backend"},
		Env:  append([]string{"GIT_PROJECT_ROOT=" + root, "GIT_HTTP_EXPORT_ALL=1"}, gitEnv...),
	}
	v2 = httptest.NewServer(backend)
	defer v2.Close()
	v0 = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Header.Del("Git-Protocol")
		backend.ServeHTTP(w, r)
	}))
	defer v0.Close()
	ctx := context.Background()
	shared, err = New(ctx, http.DefaultClient)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer func() {
		if err := shared.Close(ctx); err != nil {
			fmt.Fprintln(os.Stderr, err)
			code = 1
		}
	}()
	return m.Run()
}

func repo(server *httptest.Server, name string) string {
	return server.URL + "/" + name + ".git"
}

func rev(t *testing.T, name, revision string) string {
	t.Helper()
	command := exec.Command("git", "-C", filepath.Join(root, name+".git"), "rev-parse", revision)
	command.Env = append(os.Environ(), gitEnv...)
	out, err := command.Output()
	if err != nil {
		t.Fatalf("rev-parse %s: %v", revision, err)
	}
	return strings.TrimSpace(string(out))
}

func serve(t *testing.T, handler http.HandlerFunc) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	return server
}

func respond(t *testing.T, w http.ResponseWriter, status int, contentType string, body []byte) {
	if contentType != "" {
		w.Header().Set("Content-Type", contentType)
	}
	w.WriteHeader(status)
	if _, err := w.Write(body); err != nil {
		t.Error(err)
	}
}

func isFetch(r *http.Request) bool {
	if r.Method != http.MethodPost {
		return false
	}
	body, _ := io.ReadAll(r.Body)
	r.Body = io.NopCloser(bytes.NewReader(body))
	return bytes.Contains(body, []byte("command=fetch"))
}

func onFetch(t *testing.T, body []byte) *httptest.Server {
	t.Helper()
	return serve(t, func(w http.ResponseWriter, r *http.Request) {
		if isFetch(r) {
			respond(t, w, http.StatusOK, resultT, body)
			return
		}
		backend.ServeHTTP(w, r)
	})
}

func pkt(data []byte) []byte {
	return append(fmt.Appendf(nil, "%04x", len(data)+4), data...)
}

func packfile(pack []byte) []byte {
	body := pkt([]byte("packfile\n"))
	body = append(body, pkt(append([]byte{1}, pack...))...)
	return append(body, "0000"...)
}

func sha1Pack(objects ...[]byte) []byte {
	pack := []byte("PACK")
	pack = binary.BigEndian.AppendUint32(pack, 2)
	pack = binary.BigEndian.AppendUint32(pack, uint32(len(objects)))
	for _, object := range objects {
		pack = append(pack, object...)
	}
	sum := sha1.Sum(pack)
	return append(pack, sum[:]...)
}

func packedTag(t *testing.T, raw []byte) []byte {
	t.Helper()
	header := []byte{0x80 | 4<<4 | byte(len(raw)&15)}
	for rest := len(raw) >> 4; rest > 0; rest >>= 7 {
		next := byte(rest & 0x7f)
		if rest > 0x7f {
			next |= 0x80
		}
		header = append(header, next)
	}
	var compressed bytes.Buffer
	writer := zlib.NewWriter(&compressed)
	if _, err := writer.Write(raw); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	return append(header, compressed.Bytes()...)
}

func catTag(t *testing.T, name string) []byte {
	t.Helper()
	command := exec.Command("git", "-C", filepath.Join(root, "sha1.git"), "cat-file", "tag", name)
	command.Env = append(os.Environ(), gitEnv...)
	out, err := command.Output()
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func wantCode(t *testing.T, err error, code Code, status int, detail string) {
	t.Helper()
	var failure *Error
	if !errors.As(err, &failure) {
		t.Fatalf("error = %v, want *Error %s", err, code)
	}
	if *failure != (Error{Code: code, Status: status, Detail: detail}) {
		t.Fatalf("error = %#v, want code %s status %d detail %q", failure, code, status, detail)
	}
}

func TestListRefs(t *testing.T) {
	ctx := context.Background()
	list, err := shared.ListRefs(ctx, repo(v2, "sha1"))
	if err != nil {
		t.Fatal(err)
	}
	if list.ObjectFormat != SHA1 || list.Prefixes != nil || len(list.Refs) != 11 {
		t.Fatalf("list = %+v", list)
	}
	if got, want := list.Refs["HEAD"], (Ref{OID: rev(t, "sha1", "main"), SymrefTarget: "refs/heads/main"}); got != want {
		t.Fatalf("HEAD = %+v, want %+v", got, want)
	}
	if got, want := list.Refs["refs/tags/annotated"], (Ref{OID: rev(t, "sha1", "annotated"), Peeled: rev(t, "sha1", "main")}); got != want {
		t.Fatalf("annotated = %+v, want %+v", got, want)
	}
	tags, err := shared.ListRefs(ctx, repo(v2, "sha1"), "refs/tags/v")
	if err != nil {
		t.Fatal(err)
	}
	if len(tags.Refs) != 1 || !reflect.DeepEqual(tags.Prefixes, []string{"refs/tags/v"}) {
		t.Fatalf("tags = %+v", tags)
	}
	empty, err := shared.ListRefs(ctx, repo(v2, "empty"))
	if err != nil {
		t.Fatal(err)
	}
	if len(empty.Refs) != 0 || empty.Unborn["HEAD"] != "refs/heads/main" {
		t.Fatalf("empty = %+v", empty)
	}
	sha256, err := shared.ListRefs(ctx, repo(v2, "sha256"))
	if err != nil {
		t.Fatal(err)
	}
	if sha256.ObjectFormat != SHA256 {
		t.Fatalf("format = %s", sha256.ObjectFormat)
	}
}

func TestProtocolV0(t *testing.T) {
	ctx := context.Background()
	old, err := shared.ListRefs(ctx, repo(v0, "sha1"))
	if err != nil {
		t.Fatal(err)
	}
	current, err := shared.ListRefs(ctx, repo(v2, "sha1"))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(old, current) {
		t.Fatalf("v0 = %+v\nv2 = %+v", old, current)
	}
	heads, err := shared.ListRefs(ctx, repo(v0, "sha1"), "refs/heads/")
	if err != nil {
		t.Fatal(err)
	}
	if len(heads.Refs) != 4 {
		t.Fatalf("heads = %+v", heads.Refs)
	}
	_, err = shared.GetObject(ctx, repo(v0, "sha1"), rev(t, "sha1", "main"))
	wantCode(t, err, CodeUnsupportedFeature, 0, "Git protocol v2")
	_, err = shared.ResolveObject(ctx, repo(v0, "sha1"), "main")
	wantCode(t, err, CodeUnsupportedFeature, 0, "Git protocol v2")
}

func TestGetRef(t *testing.T) {
	ctx := context.Background()
	ref, ok, err := shared.GetRef(ctx, repo(v2, "sha1"), "refs/heads/heads/x")
	if err != nil || !ok || ref.OID != rev(t, "sha1", "refs/heads/heads/x") {
		t.Fatalf("GetRef = %+v %v %v", ref, ok, err)
	}
	if _, ok, err := shared.GetRef(ctx, repo(v2, "sha1"), "heads/x"); ok || err != nil {
		t.Fatalf("GetRef heads/x = %v %v", ok, err)
	}
	_, _, err = shared.GetRef(ctx, repo(v2, "missing"), "HEAD")
	wantCode(t, err, CodeNotFound, http.StatusNotFound, "")
}

func TestResolve(t *testing.T) {
	ctx := context.Background()
	found, err := shared.Resolve(ctx, repo(v2, "sha1"), "v2")
	if err != nil {
		t.Fatal(err)
	}
	want := &RefMatch{Name: "refs/tags/v2", Ref: Ref{OID: rev(t, "sha1", "refs/tags/v2")}, Others: []string{"refs/heads/v2"}}
	if !reflect.DeepEqual(found, want) {
		t.Fatalf("Resolve v2 = %+v", found)
	}
	main := rev(t, "sha1", "main")
	object, err := shared.Resolve(ctx, repo(v2, "sha1"), main)
	if err != nil || !reflect.DeepEqual(object, &ObjectMatch{OID: main, Refs: []string{}}) {
		t.Fatalf("Resolve oid = %+v %v", object, err)
	}
	missing, err := shared.Resolve(ctx, repo(v2, "sha1"), "missing")
	if err != nil || !reflect.DeepEqual(missing, &NotFound{}) {
		t.Fatalf("Resolve missing = %+v %v", missing, err)
	}
	_, err = shared.Resolve(ctx, repo(v2, "sha1"), "v2^{}")
	wantCode(t, err, CodeInvalidRefName, 0, "")
}

func TestResolveObject(t *testing.T) {
	var requests atomic.Int32
	server := serve(t, func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		backend.ServeHTTP(w, r)
	})
	ctx := context.Background()
	resolved, err := shared.ResolveObject(ctx, repo(server, "sha1"), "annotated")
	if err != nil {
		t.Fatal(err)
	}
	if match, ok := resolved.Resolution.(*RefMatch); !ok || match.Name != "refs/tags/annotated" {
		t.Fatalf("resolution = %#v, want annotated ref", resolved.Resolution)
	}
	if tag, ok := resolved.Object.(*Tag); !ok || tag.OID != rev(t, "sha1", "refs/tags/annotated") {
		t.Fatalf("object = %#v, want annotated tag", resolved.Object)
	}
	if count := requests.Swap(0); count != 3 {
		t.Fatalf("requests = %d, want 3", count)
	}
	resolved, err = shared.ResolveObject(ctx, repo(server, "sha1"), "missing")
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := resolved.Resolution.(*NotFound); !ok || resolved.Object != nil {
		t.Fatalf("resolved = %#v, want NotFound without an object", resolved)
	}
	if count := requests.Load(); count != 2 {
		t.Fatalf("requests = %d, want 2", count)
	}

	oid := rev(t, "sha256", "main")
	resolved, err = shared.ResolveObject(ctx, repo(v2, "sha256"), strings.ToUpper(oid))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(resolved.Resolution, &ObjectMatch{OID: oid, Refs: []string{}}) {
		t.Fatalf("resolution = %#v, want object ID %s", resolved.Resolution, oid)
	}
	if commit, ok := resolved.Object.(*Commit); !ok || commit.OID != oid {
		t.Fatalf("object = %#v, want commit %s", resolved.Object, oid)
	}

	unknown := strings.Repeat("1", 40)
	resolved, err = shared.ResolveObject(ctx, repo(v2, "sha1"), unknown)
	wantCode(t, err, CodeRemote, 0, "upload-pack: not our ref "+unknown)
	if resolved != nil {
		t.Fatalf("resolved = %#v, want nil on fetch failure", resolved)
	}
}

func TestResolveObjectPinsAdvertisedOID(t *testing.T) {
	ctx := context.Background()
	replacement := rev(t, "moving", "main")
	for _, branch := range []string{"moved", "pruned"} {
		t.Run(branch, func(t *testing.T) {
			dir, err := os.MkdirTemp(root, "moving-")
			if err != nil {
				t.Fatal(err)
			}
			name := filepath.Base(dir) + "/repo"
			path := filepath.Join(dir, "repo.git")
			if err := os.CopyFS(path, os.DirFS(filepath.Join(root, "moving.git"))); err != nil {
				t.Fatal(err)
			}
			original := rev(t, name, branch)
			ref := "refs/heads/" + branch
			var listings atomic.Int32
			server := serve(t, func(w http.ResponseWriter, r *http.Request) {
				if r.Method != http.MethodPost || isFetch(r) || listings.Add(1) != 1 {
					backend.ServeHTTP(w, r)
					return
				}

				listing := httptest.NewRecorder()
				backend.ServeHTTP(listing, r)
				runGit := func(args ...string) {
					command := exec.Command("git", append([]string{"-C", path}, args...)...)
					command.Env = append(os.Environ(), gitEnv...)
					if out, err := command.CombinedOutput(); err != nil {
						t.Errorf("git %v: %v: %s", args, err, out)
					}
				}
				runGit("update-ref", ref, replacement)
				if branch == "pruned" {
					runGit("prune", "--expire=now")
				}
				respond(t, w, listing.Code, listing.Header().Get("Content-Type"), listing.Body.Bytes())
			})
			resolved, err := shared.ResolveObject(ctx, repo(server, name), branch)
			if branch == "pruned" {
				wantCode(t, err, CodeRemote, 0, "upload-pack: not our ref "+original)
				if resolved != nil {
					t.Fatalf("resolved = %#v, want nil after pruning %s", resolved, original)
				}
			} else {
				if err != nil {
					t.Fatal(err)
				}
				if match, ok := resolved.Resolution.(*RefMatch); !ok || match.Name != ref || match.Ref.OID != original {
					t.Fatalf("resolution = %#v, want %s at %s", resolved.Resolution, ref, original)
				}
				if commit, ok := resolved.Object.(*Commit); !ok || commit.OID != original {
					t.Fatalf("object = %#v, want commit %s", resolved.Object, original)
				}
			}
			if count := listings.Load(); count != 1 {
				t.Fatalf("listings = %d, want 1", count)
			}
		})
	}
}

func TestResolveObjectCancelledDuringFetch(t *testing.T) {
	ctx := context.Background()
	cancelled, cancel := context.WithCancel(ctx)
	defer cancel()
	server := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if isFetch(r) {
			cancel()
			<-r.Context().Done()
			return
		}
		backend.ServeHTTP(w, r)
	})
	resolved, err := shared.ResolveObject(cancelled, repo(server, "sha1"), "main")
	if resolved != nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("ResolveObject = (%#v, %v), want (nil, context.Canceled)", resolved, err)
	}
	resolved, err = shared.ResolveObject(ctx, repo(v2, "sha1"), "main")
	if err != nil || resolved == nil || resolved.Object == nil {
		t.Fatalf("ResolveObject after cancellation = (%#v, %v)", resolved, err)
	}
}

func TestResolveObjectFetchTransportFailure(t *testing.T) {
	server := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if !isFetch(r) {
			backend.ServeHTTP(w, r)
			return
		}
		connection, _, err := http.NewResponseController(w).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		if err := connection.Close(); err != nil {
			t.Error(err)
		}
	})
	resolved, err := shared.ResolveObject(context.Background(), repo(server, "sha1"), "main")
	if resolved != nil || !errors.Is(err, io.EOF) {
		t.Fatalf("ResolveObject = (%#v, %v), want (nil, io.EOF)", resolved, err)
	}
}

func TestGetObject(t *testing.T) {
	ctx := context.Background()
	main := rev(t, "sha1", "main")
	object, err := shared.GetObject(ctx, repo(v2, "sha1"), strings.ToUpper(main))
	if err != nil {
		t.Fatal(err)
	}
	commit, ok := object.(*Commit)
	if !ok || commit.ObjectID() != main || commit.Kind() != KindCommit {
		t.Fatalf("object = %+v", object)
	}
	if commit.Tree != rev(t, "sha1", "main^{tree}") || !reflect.DeepEqual(commit.Parents, []string{rev(t, "sha1", "main~1")}) {
		t.Fatalf("commit = %+v", commit)
	}
	when := time.Date(2026, 1, 2, 3, 4, 5, 0, time.FixedZone("", 2*60*60))
	if commit.Author.Name != "Ada Lovelace" || commit.Committer.Email != "ada@example.com" || !commit.Author.When.Equal(when) {
		t.Fatalf("author = %+v", commit.Author)
	}
	if _, offset := commit.Committer.When.Zone(); offset != 2*60*60 {
		t.Fatalf("offset = %d", offset)
	}
	if commit.Message != "second\n\nwith a body\n" || commit.Encoding != "" {
		t.Fatalf("message = %q encoding = %q", commit.Message, commit.Encoding)
	}
	blob, err := shared.GetObject(ctx, repo(v2, "sha1"), rev(t, "sha1", "main:file"))
	if err != nil || !reflect.DeepEqual(blob, &Blob{OID: rev(t, "sha1", "main:file"), Data: []byte("one\n")}) || blob.Kind() != KindBlob || blob.ObjectID() != rev(t, "sha1", "main:file") {
		t.Fatalf("blob = %+v %v", blob, err)
	}
	tree, err := shared.GetObject(ctx, repo(v2, "sha256"), rev(t, "sha256", "main^{tree}"))
	if err != nil {
		t.Fatal(err)
	}
	entries := tree.(*Tree).Entries
	if tree.Kind() != KindTree || tree.ObjectID() != rev(t, "sha256", "main^{tree}") || len(entries) != 4 || entries[1] != (TreeEntry{Mode: 0o100644, Name: "file", OID: rev(t, "sha256", "main:file")}) {
		t.Fatalf("tree = %+v", tree)
	}
	tag, err := shared.GetObject(ctx, repo(v2, "sha1"), rev(t, "sha1", "annotated"))
	if err != nil {
		t.Fatal(err)
	}
	annotated := tag.(*Tag)
	if tag.Kind() != KindTag || annotated.Target != main || annotated.TargetKind != KindCommit || annotated.Name != "annotated" || annotated.Message != "annotated\n" || annotated.Tagger == nil || annotated.Tagger.Name != "Ada Lovelace" {
		t.Fatalf("tag = %+v", annotated)
	}
}

func TestNonUTF8Bytes(t *testing.T) {
	ctx := context.Background()
	object, err := shared.GetObject(ctx, repo(v2, "latin1"), rev(t, "latin1", "main"))
	if err != nil {
		t.Fatal(err)
	}
	commit := object.(*Commit)
	if commit.Encoding != "ISO-8859-1" || commit.Message != "Caf\xe9\n" || commit.Author.Name != "\xc9mile" || commit.Committer.Name != "Ada Lovelace" {
		t.Fatalf("commit = %+v", commit)
	}
	tree, err := shared.GetObject(ctx, repo(v2, "latin1"), commit.Tree)
	if err != nil {
		t.Fatal(err)
	}
	if name := tree.(*Tree).Entries[0].Name; name != "caf\xe9" {
		t.Fatalf("name = %q", name)
	}
}

func TestPeel(t *testing.T) {
	ctx := context.Background()
	var requests atomic.Int32
	counting := serve(t, func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		backend.ServeHTTP(w, r)
	})
	peeled, err := shared.Peel(ctx, repo(counting, "sha1"), rev(t, "sha1", "nested"))
	if err != nil {
		t.Fatal(err)
	}
	if requests.Load() != 2 || len(peeled.Chain) != 3 || peeled.Chain[1].ObjectID() != rev(t, "sha1", "annotated") || peeled.Target.ObjectID() != rev(t, "sha1", "main") {
		t.Fatalf("requests = %d peeled = %+v", requests.Load(), peeled)
	}
	tree, err := shared.Peel(ctx, repo(v2, "sha1"), rev(t, "sha1", "tree-tag"))
	if err != nil || tree.Target.Kind() != KindTree {
		t.Fatalf("tree-tag = %+v %v", tree, err)
	}
	var served atomic.Bool
	tagOnly := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if isFetch(r) && !served.Swap(true) {
			respond(t, w, http.StatusOK, resultT, packfile(sha1Pack(packedTag(t, catTag(t, "annotated")))))
			return
		}
		backend.ServeHTTP(w, r)
	})
	split, err := shared.Peel(ctx, repo(tagOnly, "sha1"), rev(t, "sha1", "annotated"))
	if err != nil || len(split.Chain) != 2 || split.Target.ObjectID() != rev(t, "sha1", "main") {
		t.Fatalf("split = %+v %v", split, err)
	}
	longest, err := shared.Peel(ctx, repo(v2, "deep"), rev(t, "deep", "t14"))
	if err != nil || len(longest.Chain) != 16 {
		t.Fatalf("t14 = %+v %v", longest, err)
	}
	_, err = shared.Peel(ctx, repo(v2, "deep"), rev(t, "deep", "t16"))
	wantCode(t, err, CodeLimitExceeded, 0, "tag chain longer than 16 objects")
	_, err = shared.Peel(ctx, repo(v2, "sha1"), "abc")
	wantCode(t, err, CodeInvalidObjectID, 0, "")
}

func TestResponseLimit(t *testing.T) {
	const contentType = "application/x-git-upload-pack-advertisement"
	advertisement := append(pkt([]byte("version 2\n")), "0000"...)
	ctx := context.Background()
	client, err := NewWithOptions(ctx, http.DefaultClient, ClientOptions{MaxResponseBytes: int64(len(advertisement))})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := client.Close(ctx); err != nil {
			t.Error(err)
		}
	})
	for _, tc := range []struct {
		name   string
		status int
		body   []byte
		code   Code
		detail string
	}{
		{"exact", http.StatusOK, advertisement, CodeUnsupportedFeature, "ls-refs"},
		{"oversized", http.StatusOK, append(bytes.Clone(advertisement), 'x'), CodeLimitExceeded, fmt.Sprintf("HTTP response exceeds %d bytes", len(advertisement))},
		{"http error", http.StatusNotFound, append(bytes.Clone(advertisement), 'x'), CodeNotFound, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server := serve(t, func(w http.ResponseWriter, _ *http.Request) {
				w.Header().Set("Content-Type", contentType)
				w.WriteHeader(tc.status)
				w.(http.Flusher).Flush()
				if _, err := w.Write(tc.body); err != nil {
					t.Error(err)
				}
			})
			_, err := client.ListRefs(ctx, repo(server, "sha1"))
			status := 0
			if tc.status != http.StatusOK {
				status = tc.status
			}
			wantCode(t, err, tc.code, status, tc.detail)
		})
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) {
	return f(r)
}

type responseLimitBody struct {
	readErr  error
	closeErr error
	bytes    int
	closed   bool
}

func (b *responseLimitBody) Read(p []byte) (int, error) {
	clear(p)
	b.bytes += len(p)
	return len(p), b.readErr
}

func (b *responseLimitBody) Close() error {
	b.closed = true
	return b.closeErr
}

func TestResponseLimitWithBodyErrors(t *testing.T) {
	readErr := errors.New("read failed after delivering bytes")
	closeErr := errors.New("close failed")
	body := &responseLimitBody{readErr: readErr, closeErr: closeErr}
	s := &session{
		ctx:              context.Background(),
		maxResponseBytes: 5,
		http: &http.Client{Transport: roundTripFunc(func(*http.Request) (*http.Response, error) {
			return &http.Response{StatusCode: http.StatusOK, Body: body, ContentLength: -1}, nil
		})},
	}
	_, err := s.exchange(git.Request{URL: "https://example.com/repo"})
	wantCode(t, err, CodeLimitExceeded, 0, "HTTP response exceeds 5 bytes")
	if !errors.Is(err, readErr) || !errors.Is(err, closeErr) {
		t.Fatalf("expected read and close errors, got %v", err)
	}
	if body.bytes != 6 || !body.closed {
		t.Fatalf("read %d bytes, closed = %t", body.bytes, body.closed)
	}
}

func TestResponseLimitOptions(t *testing.T) {
	if shared.maxResponseBytes != defaultMaxResponseBytes {
		t.Fatalf("default limit = %d, want %d", shared.maxResponseBytes, defaultMaxResponseBytes)
	}
	for _, limit := range []int64{-1, math.MaxInt64} {
		client, err := NewWithOptions(context.Background(), http.DefaultClient, ClientOptions{MaxResponseBytes: limit})
		want := fmt.Sprintf("git-remote-refs: invalid MaxResponseBytes: %d", limit)
		if client != nil || err == nil || err.Error() != want {
			t.Fatalf("NewWithOptions(%d) = %v, %v, want nil, %q", limit, client, err, want)
		}
	}
}

func TestErrors(t *testing.T) {
	ctx := context.Background()
	_, err := shared.ListRefs(ctx, "ssh://example.com/repo")
	wantCode(t, err, CodeInvalidURL, 0, "")
	_, err = shared.ListRefs(ctx, repo(v2, "sha1"), "a b")
	wantCode(t, err, CodeInvalidPrefix, 0, "")
	_, err = shared.GetObject(ctx, repo(v2, "sha1"), strings.Repeat("1", 40))
	wantCode(t, err, CodeRemote, 0, "upload-pack: not our ref "+strings.Repeat("1", 40))
	_, err = shared.GetObject(ctx, repo(v2, "sha256"), rev(t, "sha1", "main"))
	wantCode(t, err, CodeUnsupportedObjectFormat, 0, "")

	for _, tc := range []struct {
		status      int
		contentType string
		body        string
		code        Code
		want        int
		detail      string
	}{
		{http.StatusUnauthorized, "", "", CodeAuthRequired, 401, ""},
		{http.StatusInternalServerError, "", "", CodeHTTP, 500, ""},
		{http.StatusOK, "text/html", "<html>", CodeNotSmartHTTP, 0, ""},
		{http.StatusOK, "application/x-git-upload-pack-advertisement", "zzzz", CodeProtocol, 0, "invalid packet length"},
	} {
		served := serve(t, func(w http.ResponseWriter, _ *http.Request) {
			respond(t, w, tc.status, tc.contentType, []byte(tc.body))
		})
		_, err := shared.ListRefs(ctx, repo(served, "sha1"))
		wantCode(t, err, tc.code, tc.want, tc.detail)
	}

	main := rev(t, "sha1", "main")
	empty := append(append([]byte("PACK"), 0, 0, 0, 2, 0, 0, 0, 0), make([]byte, 20)...)
	_, err = shared.GetObject(ctx, repo(onFetch(t, packfile(empty)), "sha1"), main)
	wantCode(t, err, CodeChecksumMismatch, 0, "")
	huge := binary.BigEndian.AppendUint32(append([]byte("PACK"), 0, 0, 0, 2), 200_000)
	huge = append(huge, make([]byte, 20)...)
	_, err = shared.GetObject(ctx, repo(onFetch(t, packfile(huge)), "sha1"), main)
	wantCode(t, err, CodeLimitExceeded, 0, "object count")
	other := sha1Pack(packedTag(t, catTag(t, "annotated")))
	_, err = shared.GetObject(ctx, repo(onFetch(t, packfile(other)), "sha1"), main)
	wantCode(t, err, CodeMissingObject, 0, main)

	hangUp := serve(t, func(w http.ResponseWriter, _ *http.Request) {
		connection, _, err := http.NewResponseController(w).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		if err := connection.Close(); err != nil {
			t.Error(err)
		}
	})
	_, err = shared.ListRefs(ctx, repo(hangUp, "sha1"))
	var failure *Error
	if err == nil || errors.As(err, &failure) {
		t.Fatalf("transport error = %v", err)
	}
	postHangUp := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			backend.ServeHTTP(w, r)
			return
		}
		connection, _, err := http.NewResponseController(w).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		if err := connection.Close(); err != nil {
			t.Error(err)
		}
	})
	if _, err := shared.ListRefs(ctx, repo(postHangUp, "sha1")); err == nil || errors.As(err, &failure) {
		t.Fatalf("ls-refs transport error = %v", err)
	}
	if _, err := shared.GetObject(ctx, repo(postHangUp, "sha1"), main); err == nil || errors.As(err, &failure) {
		t.Fatalf("fetch transport error = %v", err)
	}
	truncated := serve(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Length", "100")
		if _, err := w.Write([]byte("00")); err != nil {
			t.Error(err)
		}
	})
	_, err = shared.ListRefs(ctx, repo(truncated, "sha1"))
	if !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("body error = %v", err)
	}
}

func TestCancelledCallKeepsClientUsable(t *testing.T) {
	ctx := context.Background()
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := shared.ListRefs(cancelled, repo(v2, "sha1")); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled ListRefs = %v", err)
	}
	if _, err := shared.ListRefs(ctx, repo(v2, "sha1")); err != nil {
		t.Fatalf("ListRefs after cancellation = %v", err)
	}
	if _, err := shared.ParseAdvertisement(cancelled, nil); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled ParseAdvertisement = %v", err)
	}
}

func TestErrorText(t *testing.T) {
	for _, tc := range []struct {
		err  *Error
		want string
	}{
		{&Error{Code: CodeRemote, Detail: "not our ref"}, "git-remote-refs: remote: not our ref"},
		{&Error{Code: CodeNotFound, Status: 404}, "git-remote-refs: not-found (HTTP 404)"},
		{&Error{Code: CodeInvalidURL}, "git-remote-refs: invalid-url"},
	} {
		if got := tc.err.Error(); got != tc.want {
			t.Errorf("Error() = %q, want %q", got, tc.want)
		}
	}
}

func TestParseAdvertisement(t *testing.T) {
	ctx := context.Background()
	for _, tc := range []struct {
		server *httptest.Server
		code   Code
	}{{v0, ""}, {v2, CodeProtocol}} {
		request, err := http.NewRequest(http.MethodGet, repo(tc.server, "sha1")+"/info/refs?service=git-upload-pack", nil)
		if err != nil {
			t.Fatal(err)
		}
		request.Header.Set("Git-Protocol", "version=2")
		resp, err := http.DefaultClient.Do(request)
		if err != nil {
			t.Fatal(err)
		}
		body, err := io.ReadAll(resp.Body)
		if err = errors.Join(err, resp.Body.Close()); err != nil {
			t.Fatal(err)
		}
		list, err := shared.ParseAdvertisement(ctx, body)
		if tc.code != "" {
			wantCode(t, err, tc.code, 0, "expected a protocol v0 advertisement")
			continue
		}
		if err != nil || list.Refs["refs/heads/main"].OID != rev(t, "sha1", "main") {
			t.Fatalf("list = %+v %v", list, err)
		}
	}
}

func TestConcurrentCalls(t *testing.T) {
	ctx := context.Background()
	main := rev(t, "sha1", "main")
	var group sync.WaitGroup
	failures := make(chan error, 16)
	for range 8 {
		group.Go(func() {
			if _, err := shared.ListRefs(ctx, repo(v2, "sha1")); err != nil {
				failures <- err
			}
		})
		group.Go(func() {
			if _, err := shared.GetObject(ctx, repo(v2, "sha1"), main); err != nil {
				failures <- err
			}
		})
	}
	group.Wait()
	close(failures)
	for err := range failures {
		t.Error(err)
	}
}

func TestLoadFailures(t *testing.T) {
	ctx := context.Background()
	original := component
	t.Cleanup(func() { component = original })
	for _, tc := range []struct {
		name  string
		bytes string
		want  string
	}{
		{"not wasm", "not wasm", ""},
		{"trapping start", "\x00asm\x0d\x00\x01\x00\x01\x2d\x00asm\x01\x00\x00\x00\x01\x04\x01\x60\x00\x00\x03\x02\x01\x00\x08\x01\x00\x0a\x05\x01\x03\x00\x00\x0b\x00\x0f\x04name\x00\x02\x01m\x01\x04\x01\x00\x01s\x02\x04\x01\x00\x00\x00\x00\x17\x0e\x63omponent-name\x01\x06\x00\x11\x01\x00\x01m", ""},
		{"no export", "\x00asm\x0d\x00\x01\x00", "does not export kjanat:git-remote-refs/git"},
	} {
		component = []byte(tc.bytes)
		_, err := New(ctx, http.DefaultClient)
		if err == nil || !strings.Contains(err.Error(), tc.want) {
			t.Errorf("%s: New = %v", tc.name, err)
		}
	}
	closed := &Client{http: http.DefaultClient, engine: wacogo.NewEngine(ctx)}
	if err := closed.engine.Close(ctx); err != nil {
		t.Fatal(err)
	}
	if err := closed.load(ctx); err == nil {
		t.Error("load succeeded on a closed engine")
	}
	component = original
	stopped, err := New(ctx, http.DefaultClient)
	if err != nil {
		t.Fatal(err)
	}
	if err := stopped.factory.Close(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := stopped.ListRefs(ctx, repo(v2, "sha1")); err == nil {
		t.Error("ListRefs succeeded with a closed factory")
	}
	if err := stopped.Close(ctx); err != nil {
		t.Error(err)
	}
}

var errInjected = errors.New("injected")

type fault struct {
	method string
	call   int
	result any
	err    error
	calls  map[string]int
}

func (f *fault) trip(method string) bool {
	f.calls[method]++
	return method == f.method && f.calls[method] == f.call
}

func inject[T any](f *fault, method string, real func() (T, error)) (T, error) {
	if !f.trip(method) {
		return real()
	}
	result, _ := f.result.(T)
	return result, f.err
}

type faultyGit struct {
	git.Git
	f *fault
}

func (g faultyGit) Candidates(ctx context.Context, name string) (git.ResultListStringError, error) {
	return inject(g.f, "Candidates", func() (git.ResultListStringError, error) { return g.Git.Candidates(ctx, name) })
}

func (g faultyGit) ParseAdvertisement(ctx context.Context, body []uint8) (git.ResultRefListError, error) {
	return inject(g.f, "ParseAdvertisement", func() (git.ResultRefListError, error) { return g.Git.ParseAdvertisement(ctx, body) })
}

func (g faultyGit) NewRemote(ctx context.Context, url string) (git.ResultRemoteError, error) {
	result, err := inject(g.f, "NewRemote", func() (git.ResultRemoteError, error) { return g.Git.NewRemote(ctx, url) })
	if ok, isOk := result.(git.ResultRemoteErrorOk); isOk && err == nil {
		return git.ResultRemoteErrorOk{Value: git.NewRemoteHandle(faultyRemote{ok.Value, g.f})}, nil
	}
	return result, err
}

type faultyRemote struct {
	real *git.RemoteHandle
	f    *fault
}

func (r faultyRemote) Drop(ctx context.Context) error { return r.real.Drop(ctx) }

func (r faultyRemote) AcceptDiscovery(ctx context.Context, response git.Response) (git.Result_Error, error) {
	return inject(r.f, "AcceptDiscovery", func() (git.Result_Error, error) { return r.real.AcceptDiscovery(ctx, response) })
}

func (r faultyRemote) AcceptFetch(ctx context.Context, response git.Response, oids []string) (git.ResultFetchedError, error) {
	return inject(r.f, "AcceptFetch", func() (git.ResultFetchedError, error) { return r.real.AcceptFetch(ctx, response, oids) })
}

func (r faultyRemote) AcceptList(ctx context.Context, response git.Response) (git.Result_Error, error) {
	return inject(r.f, "AcceptList", func() (git.Result_Error, error) { return r.real.AcceptList(ctx, response) })
}

func (r faultyRemote) DiscoveryRequest(ctx context.Context) (git.Request, error) {
	return inject(r.f, "DiscoveryRequest", func() (git.Request, error) { return r.real.DiscoveryRequest(ctx) })
}

func (r faultyRemote) FetchRequest(ctx context.Context, oids []string) (git.ResultRequestError, error) {
	return inject(r.f, "FetchRequest", func() (git.ResultRequestError, error) { return r.real.FetchRequest(ctx, oids) })
}

func (r faultyRemote) ListRequest(ctx context.Context, prefixes []string) (git.ResultOptionRequestError, error) {
	return inject(r.f, "ListRequest", func() (git.ResultOptionRequestError, error) { return r.real.ListRequest(ctx, prefixes) })
}

func (r faultyRemote) Refs(ctx context.Context) (git.ResultRefListError, error) {
	return inject(r.f, "Refs", func() (git.ResultRefListError, error) { return r.real.Refs(ctx) })
}

func (r faultyRemote) Resolve(ctx context.Context, name string) (git.ResultResolutionError, error) {
	return inject(r.f, "Resolve", func() (git.ResultResolutionError, error) { return r.real.Resolve(ctx, name) })
}

func TestComponentFailures(t *testing.T) {
	ctx := context.Background()
	main := rev(t, "sha1", "main")
	annotated := rev(t, "sha1", "annotated")
	var served atomic.Bool
	tagOnly := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if isFetch(r) && !served.Swap(true) {
			respond(t, w, http.StatusOK, resultT, packfile(sha1Pack(packedTag(t, catTag(t, "annotated")))))
			return
		}
		backend.ServeHTTP(w, r)
	})
	operations := map[string]func(*Client) error{
		"list": func(c *Client) error { _, err := c.ListRefs(ctx, repo(v2, "sha1")); return err },
		"resolve": func(c *Client) error {
			_, err := c.Resolve(ctx, repo(v2, "sha1"), "v2")
			return err
		},
		"object": func(c *Client) error { _, err := c.GetObject(ctx, repo(v2, "sha1"), main); return err },
		"peel": func(c *Client) error {
			served.Store(false)
			_, err := c.Peel(ctx, repo(tagOnly, "sha1"), annotated)
			return err
		},
		"advertisement": func(c *Client) error { _, err := c.ParseAdvertisement(ctx, []byte("0000")); return err },
	}
	unexpected := "unexpected component value"
	for _, tc := range []struct {
		operation string
		method    string
		call      int
		result    any
		err       error
		want      string
	}{
		{"resolve", "Candidates", 1, nil, errInjected, ""},
		{"resolve", "Candidates", 1, nil, nil, unexpected},
		{"advertisement", "ParseAdvertisement", 1, nil, errInjected, ""},
		{"advertisement", "ParseAdvertisement", 1, nil, nil, unexpected},
		{"list", "NewRemote", 1, nil, errInjected, ""},
		{"list", "NewRemote", 1, nil, nil, unexpected},
		{"list", "DiscoveryRequest", 1, nil, errInjected, ""},
		{"list", "DiscoveryRequest", 1, git.Request{URL: "http://[::1"}, nil, "missing ']' in host"},
		{"list", "AcceptDiscovery", 1, nil, errInjected, ""},
		{"list", "AcceptDiscovery", 1, nil, nil, unexpected},
		{"list", "ListRequest", 1, nil, errInjected, ""},
		{"list", "ListRequest", 1, nil, nil, unexpected},
		{"list", "AcceptList", 1, nil, errInjected, ""},
		{"list", "AcceptList", 1, nil, nil, unexpected},
		{"list", "AcceptList", 1, git.Result_ErrorErr{Value: git.ErrorNotSmartHTTP{}}, nil, "not-smart-http"},
		{"list", "Refs", 1, nil, errInjected, ""},
		{"list", "Refs", 1, nil, nil, unexpected},
		{"list", "Refs", 1, git.ResultRefListErrorErr{Value: git.ErrorProtocol{Value: "injected"}}, nil, "protocol: injected"},
		{"resolve", "ListRequest", 1, nil, errInjected, ""},
		{"resolve", "Resolve", 1, nil, errInjected, ""},
		{"resolve", "Resolve", 1, nil, nil, unexpected},
		{"resolve", "Resolve", 1, git.ResultResolutionErrorOk{}, nil, unexpected},
		{"resolve", "Resolve", 1, git.ResultResolutionErrorErr{Value: git.ErrorInvalidRefName{}}, nil, "invalid-ref-name"},
		{"object", "FetchRequest", 1, nil, errInjected, ""},
		{"object", "FetchRequest", 1, nil, nil, unexpected},
		{"object", "AcceptFetch", 1, nil, errInjected, ""},
		{"object", "AcceptFetch", 1, nil, nil, unexpected},
		{"object", "AcceptFetch", 1, git.ResultFetchedErrorOk{Value: git.Fetched{Wanted: []git.GitObject{nil}}}, nil, unexpected},
		{"object", "AcceptFetch", 1, git.ResultFetchedErrorOk{Value: git.Fetched{Wanted: []git.GitObject{git.GitObjectBlob{}}, Objects: []git.TupleStringGitObject{{F0: "x"}}}}, nil, unexpected},
		{"object", "AcceptFetch", 1, git.ResultFetchedErrorErr{}, nil, unexpected},
		{"peel", "FetchRequest", 2, nil, errInjected, ""},
	} {
		name := fmt.Sprintf("%s/%s#%d/%v/%T", tc.operation, tc.method, tc.call, tc.err, tc.result)
		f := &fault{method: tc.method, call: tc.call, result: tc.result, err: tc.err, calls: map[string]int{}}
		var err error
		withFault(f, func() { err = operations[tc.operation](shared) })
		switch {
		case tc.err != nil && !errors.Is(err, tc.err):
			t.Errorf("%s: error = %v, want %v", name, err, tc.err)
		case tc.err == nil && (err == nil || !strings.Contains(err.Error(), tc.want)):
			t.Errorf("%s: error = %v, want %q", name, err, tc.want)
		}
		if f.calls[tc.method] < tc.call {
			t.Errorf("%s: %s ran %d times", name, tc.method, f.calls[tc.method])
		}
	}

	incomplete := &fault{method: "Resolve", call: 1, result: git.ResultResolutionErrorOk{Value: git.ResolutionIncomplete{Value: []string{"refs/tags/v2"}}}, calls: map[string]int{}}
	var resolution Resolution
	var err error
	withFault(incomplete, func() { resolution, err = shared.Resolve(ctx, repo(v2, "sha1"), "v2") })
	if err != nil || !reflect.DeepEqual(resolution, &Incomplete{Missing: []string{"refs/tags/v2"}}) {
		t.Fatalf("Resolve = %+v %v", resolution, err)
	}
}

func withFault(f *fault, run func()) {
	shared.adapt = func(g git.Git) git.Git { return faultyGit{g, f} }
	defer func() { shared.adapt = nil }()
	run()
}
