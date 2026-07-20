output "name" {
  description = "The container workload name."
  value       = terraform_data.container.output.name
}

output "image" {
  description = "The container image ref."
  value       = terraform_data.container.output.image
}

output "rootless" {
  description = "Whether the container runs rootless (the default)."
  value       = terraform_data.container.output.rootless
}

output "quadlet_unit" {
  description = <<-EOT
    The rendered rootless Quadlet `.container` unit the Ansible container_host role
    installs as a systemd service. Carried in the root's `containers` output for the
    configure leg to consume.
  EOT
  value       = local.quadlet_unit
}
