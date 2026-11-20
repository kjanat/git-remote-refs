package gitremoterefs

import (
	"bytes"
	"context"
	_ "embed"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/kjanat/git-remote-refs/go/internal/kjanat/git-remote-refs/git"
	"github.com/kjanat/wacogo"
	"github.com/kjanat/wacogo/host"
)

//go:embed git-remote-refs.wasm
var component []byte

const maxTagChain = 16

type Client struct {
	http     *http.Client
	mu       sync.Mutex
	engine   *wacogo.Engine
	factory  *git.Factory
	host     *host.ComponentInstance
	instance *wacogo.ComponentInstance
	git      git.Git
}

type stub struct{ git.Git }

func New(ctx context.Context, httpClient *http.Client) (*Client, error) {
	c := &Client{http: httpClient, engine: wacogo.NewEngine(ctx)}
	if err := c.load(ctx); err != nil {
		return nil, errors.Join(err, c.Close(ctx))
	}
	return c, nil
}

func (c *Client) load(ctx context.Context) error {
	factory, err := git.NewFactory(ctx, c.engine)
	if err != nil {
		return err
	}
	c.factory = factory
	return c.start(ctx)
}

func (c *Client) start(ctx context.Context) (err error) {
	if c.host, err = c.factory.NewInstance(ctx, stub{}, nil); err != nil {
		return err
	}
	compiled, err := c.engine.LoadComponent(ctx, bytes.NewReader(component))
	if err != nil {
		return err
	}
	if c.instance, err = compiled.Instantiate(ctx); err != nil {
		return err
	}
	exported := c.instance.ExportedInstance("kjanat:git-remote-refs/git")
	if exported == nil {
		return errors.New("git-remote-refs: component does not export kjanat:git-remote-refs/git")
	}
	c.git = git.WrapInstance(c.host, exported)
	return nil
}

func (c *Client) Close(ctx context.Context) error {
	var errs []error
	if c.instance != nil {
		errs = append(errs, c.instance.Close(ctx))
	}
	if c.host != nil {
		errs = append(errs, c.host.Close(ctx))
	}
	if c.factory != nil {
		errs = append(errs, c.factory.Close(ctx))
	}
	errs = append(errs, c.engine.Close(ctx))
	return errors.Join(errs...)
}

func (c *Client) ListRefs(ctx context.Context, repository string, prefixes ...string) (*RefList, error) {
	var list *RefList
	err := c.session(ctx, repository, func(s *session) (err error) {
		list, err = s.list(prefixes)
		return err
	})
	return list, err
}

func (c *Client) GetRef(ctx context.Context, repository, name string) (Ref, bool, error) {
	list, err := c.ListRefs(ctx, repository, name)
	if err != nil {
		return Ref{}, false, err
	}
	ref, ok := list.Refs[name]
	return ref, ok, nil
}

func (c *Client) Resolve(ctx context.Context, repository, name string) (Resolution, error) {
	var resolution Resolution
	err := c.session(ctx, repository, func(s *session) error {
		names, err := c.candidates(ctx, name)
		if err != nil {
			return err
		}
		if _, err := s.list(names); err != nil {
			return err
		}
		resolution, err = s.resolve(name)
		return err
	})
	return resolution, err
}

func (c *Client) GetObject(ctx context.Context, repository, oid string) (Object, error) {
	var object Object
	err := c.session(ctx, repository, func(s *session) error {
		wanted, _, err := s.fetch(oid)
		object = wanted
		return err
	})
	return object, err
}

func (c *Client) Peel(ctx context.Context, repository, oid string) (*Peeled, error) {
	var peeled *Peeled
	err := c.session(ctx, repository, func(s *session) error {
		object, objects, err := s.fetch(oid)
		if err != nil {
			return err
		}
		chain := []Object{object}
		for {
			tag, ok := object.(*Tag)
			if !ok {
				break
			}
			if len(chain) >= maxTagChain {
				return &Error{Code: CodeLimitExceeded, Detail: fmt.Sprintf("tag chain longer than %d objects", maxTagChain)}
			}
			if known, ok := objects[tag.Target]; ok {
				object = known
			} else if object, objects, err = s.fetch(tag.Target); err != nil {
				return err
			}
			chain = append(chain, object)
		}
		peeled = &Peeled{Chain: chain, Target: object}
		return nil
	})
	return peeled, err
}

func (c *Client) ParseAdvertisement(ctx context.Context, body []byte) (*RefList, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	result, err := c.git.ParseAdvertisement(ctx, body)
	if err != nil {
		return nil, err
	}
	switch r := result.(type) {
	case git.ResultRefListErrorOk:
		return refList(r.Value), nil
	case git.ResultRefListErrorErr:
		return nil, failure(r.Value)
	}
	return nil, unexpected(result)
}

func (c *Client) candidates(ctx context.Context, name string) ([]string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	result, err := c.git.Candidates(ctx, name)
	if err != nil {
		return nil, err
	}
	switch r := result.(type) {
	case git.ResultListStringErrorOk:
		return r.Value, nil
	case git.ResultListStringErrorErr:
		return nil, failure(r.Value)
	}
	return nil, unexpected(result)
}

type session struct {
	ctx    context.Context
	client *Client
	remote *git.RemoteHandle
}

func (c *Client) session(ctx context.Context, repository string, work func(*session) error) (err error) {
	remote, err := c.newRemote(ctx, repository)
	if err != nil {
		return err
	}
	s := &session{ctx: ctx, client: c, remote: remote}
	defer func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		err = errors.Join(err, remote.Drop(ctx))
	}()
	if err := s.discover(); err != nil {
		return err
	}
	return work(s)
}

func (c *Client) newRemote(ctx context.Context, repository string) (*git.RemoteHandle, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	result, err := c.git.NewRemote(ctx, repository)
	if err != nil {
		return nil, err
	}
	switch r := result.(type) {
	case git.ResultRemoteErrorOk:
		return r.Value, nil
	case git.ResultRemoteErrorErr:
		return nil, failure(r.Value)
	}
	return nil, unexpected(result)
}

func (s *session) locked(call func() error) error {
	s.client.mu.Lock()
	defer s.client.mu.Unlock()
	return call()
}

func (s *session) discover() error {
	var request git.Request
	if err := s.locked(func() (err error) {
		request, err = s.remote.DiscoveryRequest(s.ctx)
		return err
	}); err != nil {
		return err
	}
	response, err := s.exchange(request)
	if err != nil {
		return err
	}
	return s.locked(func() error {
		result, err := s.remote.AcceptDiscovery(s.ctx, response)
		if err != nil {
			return err
		}
		return unit(result)
	})
}

func (s *session) list(prefixes []string) (*RefList, error) {
	var request git.OptionRequest
	if err := s.locked(func() error {
		result, err := s.remote.ListRequest(s.ctx, prefixes)
		if err != nil {
			return err
		}
		switch r := result.(type) {
		case git.ResultOptionRequestErrorOk:
			request = r.Value
			return nil
		case git.ResultOptionRequestErrorErr:
			return failure(r.Value)
		}
		return unexpected(result)
	}); err != nil {
		return nil, err
	}
	if request.IsSome {
		response, err := s.exchange(request.Value)
		if err != nil {
			return nil, err
		}
		if err := s.locked(func() error {
			result, err := s.remote.AcceptList(s.ctx, response)
			if err != nil {
				return err
			}
			return unit(result)
		}); err != nil {
			return nil, err
		}
	}
	var list *RefList
	err := s.locked(func() error {
		result, err := s.remote.Refs(s.ctx)
		if err != nil {
			return err
		}
		switch r := result.(type) {
		case git.ResultRefListErrorOk:
			list = refList(r.Value)
			return nil
		case git.ResultRefListErrorErr:
			return failure(r.Value)
		}
		return unexpected(result)
	})
	return list, err
}

func (s *session) resolve(name string) (Resolution, error) {
	var resolution Resolution
	err := s.locked(func() error {
		result, err := s.remote.Resolve(s.ctx, name)
		if err != nil {
			return err
		}
		switch r := result.(type) {
		case git.ResultResolutionErrorOk:
			resolution, err = resolve(r.Value)
			return err
		case git.ResultResolutionErrorErr:
			return failure(r.Value)
		}
		return unexpected(result)
	})
	return resolution, err
}

func (s *session) fetch(oid string) (Object, map[string]Object, error) {
	oids := []string{oid}
	var request git.Request
	if err := s.locked(func() error {
		result, err := s.remote.FetchRequest(s.ctx, oids)
		if err != nil {
			return err
		}
		switch r := result.(type) {
		case git.ResultRequestErrorOk:
			request = r.Value
			return nil
		case git.ResultRequestErrorErr:
			return failure(r.Value)
		}
		return unexpected(result)
	}); err != nil {
		return nil, nil, err
	}
	response, err := s.exchange(request)
	if err != nil {
		return nil, nil, err
	}
	var fetched git.Fetched
	if err := s.locked(func() error {
		result, err := s.remote.AcceptFetch(s.ctx, response, oids)
		if err != nil {
			return err
		}
		switch r := result.(type) {
		case git.ResultFetchedErrorOk:
			fetched = r.Value
			return nil
		case git.ResultFetchedErrorErr:
			return failure(r.Value)
		}
		return unexpected(result)
	}); err != nil {
		return nil, nil, err
	}
	wanted, err := object(fetched.Wanted[0])
	if err != nil {
		return nil, nil, err
	}
	objects := make(map[string]Object, len(fetched.Objects))
	for _, entry := range fetched.Objects {
		if objects[entry.F0], err = object(entry.F1); err != nil {
			return nil, nil, err
		}
	}
	return wanted, objects, nil
}

func (s *session) exchange(request git.Request) (git.Response, error) {
	method := http.MethodGet
	if request.Method == git.MethodPost {
		method = http.MethodPost
	}
	var body io.Reader
	if len(request.Body) > 0 {
		body = bytes.NewReader(request.Body)
	}
	req, err := http.NewRequestWithContext(s.ctx, method, request.URL, body)
	if err != nil {
		return git.Response{}, err
	}
	for _, header := range request.Headers {
		req.Header.Set(header.F0, header.F1)
	}
	resp, err := s.client.http.Do(req)
	if err != nil {
		return git.Response{}, err
	}
	data, err := io.ReadAll(resp.Body)
	if err = errors.Join(err, resp.Body.Close()); err != nil {
		return git.Response{}, err
	}
	response := git.Response{Status: uint16(resp.StatusCode), Body: data}
	if contentType := resp.Header.Get("Content-Type"); contentType != "" {
		response.ContentType = git.SomeString(contentType)
	}
	return response, nil
}

func unit(result git.Result_Error) error {
	switch r := result.(type) {
	case git.Result_ErrorOk:
		return nil
	case git.Result_ErrorErr:
		return failure(r.Value)
	}
	return unexpected(result)
}

func unexpected(value any) error {
	return fmt.Errorf("git-remote-refs: unexpected component value %T", value)
}

func failure(value git.Error) error {
	switch e := value.(type) {
	case git.ErrorInvalidURL:
		return &Error{Code: CodeInvalidURL}
	case git.ErrorInvalidRefName:
		return &Error{Code: CodeInvalidRefName}
	case git.ErrorInvalidPrefix:
		return &Error{Code: CodeInvalidPrefix}
	case git.ErrorInvalidObjectID:
		return &Error{Code: CodeInvalidObjectID}
	case git.ErrorAuthRequired:
		return &Error{Code: CodeAuthRequired, Status: int(e.Value)}
	case git.ErrorNotFound:
		return &Error{Code: CodeNotFound, Status: http.StatusNotFound}
	case git.ErrorHTTP:
		return &Error{Code: CodeHTTP, Status: int(e.Value)}
	case git.ErrorNotSmartHTTP:
		return &Error{Code: CodeNotSmartHTTP}
	case git.ErrorProtocol:
		return &Error{Code: CodeProtocol, Detail: e.Value}
	case git.ErrorRemote:
		return &Error{Code: CodeRemote, Detail: e.Value}
	case git.ErrorUnsupportedObjectFormat:
		return &Error{Code: CodeUnsupportedObjectFormat}
	case git.ErrorUnsupportedFeature:
		return &Error{Code: CodeUnsupportedFeature, Detail: e.Value}
	case git.ErrorChecksumMismatch:
		return &Error{Code: CodeChecksumMismatch}
	case git.ErrorLimitExceeded:
		return &Error{Code: CodeLimitExceeded, Detail: e.Value}
	case git.ErrorMissingObject:
		return &Error{Code: CodeMissingObject, Detail: e.Value}
	}
	return unexpected(value)
}

func optional(value git.OptionString) string {
	if value.IsSome {
		return value.Value
	}
	return ""
}

func text(value string, raw git.OptionListU8) string {
	if raw.IsSome {
		return string(raw.Value)
	}
	return value
}

func ref(value git.GitRef) Ref {
	return Ref{OID: value.Oid, Peeled: optional(value.Peeled), SymrefTarget: optional(value.SymrefTarget)}
}

func refList(value git.RefList) *RefList {
	list := &RefList{
		ObjectFormat: ObjectFormat(value.ObjectFormat.String()),
		Refs:         make(map[string]Ref, len(value.Refs)),
		Unborn:       make(map[string]string, len(value.Unborn)),
	}
	for _, entry := range value.Refs {
		list.Refs[entry.F0] = ref(entry.F1)
	}
	for _, entry := range value.Unborn {
		list.Unborn[entry.F0] = entry.F1
	}
	if value.Prefixes.IsSome {
		list.Prefixes = value.Prefixes.Value
	}
	return list
}

func resolve(value git.Resolution) (Resolution, error) {
	switch r := value.(type) {
	case git.ResolutionFoundRef:
		return &RefMatch{Name: r.Value.Name, Ref: ref(r.Value.Target), Others: r.Value.Others}, nil
	case git.ResolutionFoundObject:
		return &ObjectMatch{OID: r.Value.Oid, Refs: r.Value.Refs}, nil
	case git.ResolutionNotFound:
		return &NotFound{}, nil
	case git.ResolutionIncomplete:
		return &Incomplete{Missing: r.Value}, nil
	}
	return nil, unexpected(value)
}

func signature(value git.Signature) Signature {
	zone := time.FixedZone("", int(value.OffsetMinutes)*60)
	return Signature{
		Name:  text(value.Name, value.NameBytes),
		Email: text(value.Email, value.EmailBytes),
		When:  time.Unix(value.Time, 0).In(zone),
	}
}

func kind(value git.ObjectKind) Kind {
	return Kind(value.String())
}

func object(value git.GitObject) (Object, error) {
	switch o := value.(type) {
	case git.GitObjectCommit:
		return &Commit{
			OID:       o.Value.Oid,
			Tree:      o.Value.Tree,
			Parents:   o.Value.Parents,
			Author:    signature(o.Value.Author),
			Committer: signature(o.Value.Committer),
			Encoding:  optional(o.Value.Encoding),
			Message:   text(o.Value.Message, o.Value.MessageBytes),
		}, nil
	case git.GitObjectTag:
		tag := &Tag{
			OID:        o.Value.Oid,
			Target:     o.Value.Target,
			TargetKind: kind(o.Value.TargetKind),
			Name:       text(o.Value.Name, o.Value.NameBytes),
			Message:    text(o.Value.Message, o.Value.MessageBytes),
		}
		if o.Value.Tagger.IsSome {
			tagger := signature(o.Value.Tagger.Value)
			tag.Tagger = &tagger
		}
		return tag, nil
	case git.GitObjectTree:
		entries := make([]TreeEntry, len(o.Value.Entries))
		for i, entry := range o.Value.Entries {
			entries[i] = TreeEntry{Mode: entry.Mode, Name: text(entry.Name, entry.NameBytes), OID: entry.Oid}
		}
		return &Tree{OID: o.Value.Oid, Entries: entries}, nil
	case git.GitObjectBlob:
		return &Blob{OID: o.Value.Oid, Data: o.Value.Data}, nil
	}
	return nil, unexpected(value)
}
