import pytest
from decimal import Decimal
from shop.cart import line_total, discount, with_vat, money


@pytest.mark.parametrize("price,qty", [(p, q) for p in ["0.99", "1.50", "9.99", "19.90", "49.00"] for q in range(1, 61)])
def test_line_total(price, qty):
    assert line_total(price, qty) == money(Decimal(price) * qty)


@pytest.mark.parametrize("total", [Decimal(t) for t in range(5, 105, 5)])
def test_discount_ten(total):
    assert discount(total, "TEN") == money(total * Decimal("0.9"))


@pytest.mark.parametrize("total", [Decimal(t) for t in range(0, 40, 2)])
def test_discount_fiveoff_never_negative(total):
    assert discount(total, "FIVEOFF") >= 0


@pytest.mark.parametrize("country,rate", [("FR", "1.20"), ("DE", "1.19"), ("ES", "1.21"), ("IT", "1.22")])
def test_with_vat(country, rate):
    assert with_vat(Decimal("100"), country) == money(Decimal("100") * Decimal(rate))
