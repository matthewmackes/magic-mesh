output "host_count" { value = length(data.xenserver_host.all.data_items) }
output "hosts"      { value = data.xenserver_host.all.data_items }
output "sr_count"   { value = length(data.xenserver_sr.all.data_items) }
