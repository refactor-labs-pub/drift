package com.example.orders;

public class OrderRequest {
    private String customerEmail;
    private Integer totalCents;
    private String currency;

    public String getCustomerEmail() { return customerEmail; }
    public void setCustomerEmail(String v) { this.customerEmail = v; }
    public Integer getTotalCents() { return totalCents; }
    public void setTotalCents(Integer v) { this.totalCents = v; }
    public String getCurrency() { return currency; }
    public void setCurrency(String v) { this.currency = v; }
}
