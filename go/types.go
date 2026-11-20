package gitremoterefs

import (
	"fmt"
	"time"
)

type ObjectFormat string

const (
	SHA1   ObjectFormat = "sha1"
	SHA256 ObjectFormat = "sha256"
)

type Kind string

const (
	KindCommit Kind = "commit"
	KindTree   Kind = "tree"
	KindBlob   Kind = "blob"
	KindTag    Kind = "tag"
)

type Ref struct {
	OID          string
	Peeled       string
	SymrefTarget string
}

type RefList struct {
	ObjectFormat ObjectFormat
	Refs         map[string]Ref
	Unborn       map[string]string
	Prefixes     []string
}

type Resolution interface{ isResolution() }

type RefMatch struct {
	Name   string
	Ref    Ref
	Others []string
}

type ObjectMatch struct {
	OID  string
	Refs []string
}

type NotFound struct{}

type Incomplete struct {
	Missing []string
}

func (*RefMatch) isResolution()    {}
func (*ObjectMatch) isResolution() {}
func (*NotFound) isResolution()    {}
func (*Incomplete) isResolution()  {}

type Signature struct {
	Name  string
	Email string
	When  time.Time
}

type Object interface {
	ObjectID() string
	Kind() Kind
}

type Commit struct {
	OID       string
	Tree      string
	Parents   []string
	Author    Signature
	Committer Signature
	Encoding  string
	Message   string
}

type Tag struct {
	OID        string
	Target     string
	TargetKind Kind
	Name       string
	Tagger     *Signature
	Message    string
}

type TreeEntry struct {
	Mode uint32
	Name string
	OID  string
}

type Tree struct {
	OID     string
	Entries []TreeEntry
}

type Blob struct {
	OID  string
	Data []byte
}

func (c *Commit) ObjectID() string { return c.OID }
func (t *Tree) ObjectID() string   { return t.OID }
func (b *Blob) ObjectID() string   { return b.OID }
func (t *Tag) ObjectID() string    { return t.OID }

func (*Commit) Kind() Kind { return KindCommit }
func (*Tree) Kind() Kind   { return KindTree }
func (*Blob) Kind() Kind   { return KindBlob }
func (*Tag) Kind() Kind    { return KindTag }

type Peeled struct {
	Chain  []Object
	Target Object
}

type Code string

const (
	CodeInvalidURL              Code = "invalid-url"
	CodeInvalidRefName          Code = "invalid-ref-name"
	CodeInvalidPrefix           Code = "invalid-prefix"
	CodeInvalidObjectID         Code = "invalid-object-id"
	CodeAuthRequired            Code = "auth-required"
	CodeNotFound                Code = "not-found"
	CodeHTTP                    Code = "http"
	CodeNotSmartHTTP            Code = "not-smart-http"
	CodeProtocol                Code = "protocol"
	CodeRemote                  Code = "remote"
	CodeUnsupportedObjectFormat Code = "unsupported-object-format"
	CodeUnsupportedFeature      Code = "unsupported-feature"
	CodeChecksumMismatch        Code = "checksum-mismatch"
	CodeLimitExceeded           Code = "limit-exceeded"
	CodeMissingObject           Code = "missing-object"
)

type Error struct {
	Code   Code
	Status int
	Detail string
}

func (e *Error) Error() string {
	switch {
	case e.Detail != "":
		return fmt.Sprintf("git-remote-refs: %s: %s", e.Code, e.Detail)
	case e.Status != 0:
		return fmt.Sprintf("git-remote-refs: %s (HTTP %d)", e.Code, e.Status)
	default:
		return fmt.Sprintf("git-remote-refs: %s", e.Code)
	}
}
