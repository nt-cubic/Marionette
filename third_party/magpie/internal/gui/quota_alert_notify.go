//go:build !nogui

package gui

import (
	"context"
	"fmt"
	"log"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/services/notifications"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// notifier is the system's notifications, started the first time an alert
// needs them rather than with the app: started with it, a Mac build that
// isn't a signed bundle, or a Linux desktop without a notification daemon,
// would stop the app from starting at all.
type notifier struct {
	once sync.Once
	ns   *notifications.NotificationService
	err  error
	h    *host
	// refused is the Mac's answer when asked this run was no: before it
	// is asked, not yet allowed and turned off read the same
	refused atomic.Bool
}

func (n *notifier) start() error {
	n.once.Do(func() {
		ns := notifications.New()
		// on the main thread, as the app would start it: Windows registers
		// its toast activator for the thread it is on
		if err := application.InvokeSyncWithError(func() error {
			return ns.ServiceStartup(context.Background(), application.ServiceOptions{})
		}); err != nil {
			n.err = err
			log.Println("notifications:", err)
			return
		}
		// a click on an alert opens the Usage page; on a turn's end, where
		// the user goes is their agent, not magpie
		ns.OnNotificationResponse(func(r notifications.NotificationResult) {
			if strings.HasPrefix(r.Response.ID, turnNotice) {
				return
			}
			n.h.whenReady(func() { application.InvokeAsync(func() { n.h.ShowMain("usage") }) })
		})
		n.ns = ns
	})
	return n.err
}

// allowed asks for the system's leave to notify where it is asked for (the
// Mac, which asks once and then keeps the answer).
func (n *notifier) allowed() bool {
	if n.start() != nil {
		return false
	}
	if ok, err := n.ns.CheckNotificationAuthorization(); err == nil && ok {
		return true
	}
	ok, err := n.ns.RequestNotificationAuthorization()
	n.refused.Store(err == nil && !ok)
	return err == nil && ok
}

// problem is notifyProblem: why a notification wouldn't be seen.
func (n *notifier) problem() string {
	select {
	case <-n.h.ready:
	default:
		return "" // the app isn't running yet
	}
	if n.start() != nil {
		return "unavailable"
	}
	if !n.refused.Load() {
		return ""
	}
	if ok, err := n.ns.CheckNotificationAuthorization(); err == nil && !ok {
		return "denied"
	}
	return ""
}

// turnNotice is the id a turn's end is notified under, an alert's
// "magpie-alert-"
const turnNotice = "magpie-turn-"

func (n *notifier) send(title, body string) { n.sendAs("magpie-alert-", title, body) }

func (n *notifier) sendAs(kind, title, body string) {
	if !n.allowed() {
		return
	}
	id := fmt.Sprintf("%s%d", kind, time.Now().UnixNano())
	if err := n.ns.SendNotification(notifications.NotificationOptions{ID: id, Title: title, Body: body}); err != nil {
		log.Println("notifications:", err)
	}
}

// watchAlerts tells the user, with a notification, of a window used past
// the share or a balance fallen to the amount set in Settings (#368).
func (h *host) watchAlerts() {
	n := &notifier{h: h}
	wake := make(chan struct{}, 1)
	onAlerts = func() {
		// the Mac's question asked as the alert is turned on, not later
		// when one is due and the user is elsewhere
		go n.allowed()
		select {
		case wake <- struct{}{}:
		default:
		}
	}
	notifyProblem = n.problem
	go func() {
		<-h.ready
		// set before this run: asked now, so Settings can say if it was refused
		if s := settings.Load(); s.UsageAlert > 0 || s.BalanceAlert > 0 || s.ResetReminder > 0 || s.TurnDoneAlert > 0 {
			go n.allowed()
		}
		// an agent's turn done, past the length set (itieniu on X)
		gateway.TurnDone = func(t gateway.TurnEnd) {
			title, body := turnDoneText(trayLang(settings.Load().Lang, systemLang), agentName(t.Agent), t)
			n.sendAs(turnNotice, title, body)
		}
		provider.WatchQuotas(context.Background(), wake, func(a provider.QuotaAlert) {
			s := settings.Load()
			title, body := alertText(trayLang(s.Lang, systemLang), a, s.BalanceAlert, s.QuotaLeft, time.Now())
			n.send(title, body)
		})
	}()
}
