// Synthetic source referenced by the experiment TTL. The mappings are hand-written.
package fixture

type Order struct{ city, detail, alias, postcode string }
type Shop struct{ name string }
type Campaign struct{ badge string }
type Geo struct{ debug string }
type AddressVM struct{ text string }
type Context struct{ partner, aliasReady, badge bool }

func formatAddress(o Order) string    { return o.city + " " + o.detail }
func makeAddressVM(o Order) AddressVM { return AddressVM{text: formatAddress(o)} }
func pickupTitle(s Shop) string       { return s.name }
func partnerText(o Order) string      { return o.alias }
func postcodeText(o Order) string     { return o.postcode }
func prependBadge(b, s string) string { return b + " " + s }

func summaryAddress(o Order, geoLookup func() Geo) string {
	_ = geoLookup()
	return makeAddressVM(o).text
}

func selectBuilder(o Order, c Context) string {
	if c.partner {
		if c.aliasReady {
			return partnerText(o)
		}
		return postcodeText(o)
	}
	return makeAddressVM(o).text
}

func deliveryText(o Order, campaign Campaign, c Context) string {
	base := selectBuilder(o, c)
	if c.badge {
		return prependBadge(campaign.badge, base)
	}
	return base
}
