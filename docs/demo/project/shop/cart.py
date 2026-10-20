from decimal import Decimal, ROUND_HALF_UP

VAT = {"FR": Decimal("0.20"), "DE": Decimal("0.19"), "ES": Decimal("0.21"), "IT": Decimal("0.22")}


def money(x):
    return Decimal(x).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)


def line_total(price, qty):
    return money(Decimal(price) * qty)


def discount(total, code):
    if code == "TEN":
        return money(total * Decimal("0.90"))
    if code == "FIVEOFF":
        return money(max(total - 5, 0))
    return total


def with_vat(total, country):
    # Bug: Germany's rate is applied to every non-FR country.
    rate = VAT["FR"] if country == "FR" else VAT["DE"]
    return money(total * (1 + rate))
