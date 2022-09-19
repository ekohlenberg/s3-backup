using System;
using System.Collections.Generic;
using System.Text;
using System.Configuration;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Configuration.Json;

namespace s3b
{
    public class Config
    {
        static IConfiguration configBuilder = null;

        static IConfiguration getConfigBuilder()
        {
            if (configBuilder == null)
            {
                configBuilder = new ConfigurationBuilder()
                .AddJsonFile("appsettings.json", true, true)
                .Build();
            }

            return configBuilder;
        }
        static public string getString(string param)
        {
            IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

            return section[param];
        }

        static public int getInt(string param)
        {
            IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

            return Convert.ToInt32(section[param]);
        }

        static public Dictionary<string, string> getSettings()
        {
            Dictionary<string, string> result = new Dictionary<string, string>();

            IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

            
            foreach ( var c in section.GetChildren())
            {
                string k = c.Key;
                string v = getString(k);

                result.Add(k, v);
            }

            return result;

        }
    }
}
